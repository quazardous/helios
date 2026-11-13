//! Settings > Plugins: each installed plugin's own settings, rendered
//! from its configuration contract (`helios_core::config`, its manifest's
//! `[settings.<key>]`), and the store that keeps their values.
//!
//! **From the contract to the form.** Each key becomes an egui-pop
//! setting named `<plugin id>.<key>` ([`descriptor`]), placed by the
//! manifest in the plugin's section of the Plugins scope ([`section`]),
//! in the contract's order and groups:
//!
//! | contract | setting |
//! |---|---|
//! | `bool` | a toggle |
//! | `int`, `float` | a number within `min`/`max`, moved by `step` |
//! | `string` | a text |
//! | `choice` | one of its `choices` |
//! | `path` | a file or folder, with Browse |
//! | `secret` | a masked text (`sensitive`): never shown, logged or exported |
//!
//! `visible_when` becomes the field's `when` clause over the section's
//! values, `per_library` the "Set for: This library" choice. A plugin
//! whose contract has problems shows them instead of its settings.
//!
//! **Where the values live** ([`PluginSettingsStore`]):
//!
//! | value | kept in |
//! |---|---|
//! | the app's | [`PLUGINS_FILE`] in the configuration folder, by plugin then key |
//! | a library's (`per_library` keys) | the node's `project/<folder>` layer, its `plugin-settings` table |
//! | a secret | the system keyring, under `plugin:<id>` |
//!
//! A value read that the contract refuses (unknown key, wrong type, out
//! of range) is left out with its reason, shown in the Settings window.
//! A library value another client of the node changes is read again and
//! shown live.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui_pop::api::admin::model::{
    Callout, Clauses as _, Field, Records, RowField, SectionBuilder, Tone, ValueScope,
};
use egui_pop::api::admin::store::{Changes, OnChange, SettingsStore, Values, diff};
use egui_pop::model::ids::SettingId;
use egui_pop::model::values::{Descriptor, PathKind as PopPath};
use helios_connect::account::Secrets;
use helios_core::Message;
use helios_core::config::{ConfigKey, Contract, KeyType, Level, PathKind};
use serde_json::{Map, Value as Json, json};

use super::{LIBRARY, NodeLayers, NodeSettingsStore, lock};

/// The app's values of the plugins' settings, in the configuration
/// folder.
pub const PLUGINS_FILE: &str = "plugins.json";

/// The table of a library's layer holding its plugin values.
pub const LIBRARY_TABLE: &str = "plugin-settings";

/// What a secret that is set loads as: the field shows it masked, and
/// saving it unchanged keeps the secret as it is.
pub const SECRET_SET: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

/// An installed plugin as Settings > Plugins shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginSettings {
    pub id: String,
    /// Its title, in English.
    pub title: String,
    /// What it does, in English.
    pub description: String,
    /// Its own settings (the contract's first level).
    pub contract: Contract,
}

/// The plugins' settings shared between the shell's builder, which
/// reads the plugins installed, and the Settings bridge, which declares
/// them.
pub type Shared = Arc<Mutex<Vec<PluginSettings>>>;

impl PluginSettings {
    /// Every plugin the broker holds, by id.
    pub fn of(broker: &helios_plugin::Broker) -> Vec<PluginSettings> {
        let mut out: Vec<PluginSettings> = broker
            .ids()
            .filter_map(|id| {
                let m = broker.manifest(id)?;
                let title = [&m.title, &m.name]
                    .into_iter()
                    .find(|t| !t.is_empty())
                    .cloned()
                    .unwrap_or_else(|| id.to_string());
                Some(PluginSettings {
                    id: id.to_string(),
                    title,
                    description: m.description.clone(),
                    contract: m.settings.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// A key's setting id: `<plugin id>.<key>`.
    pub fn setting(&self, key: &str) -> String {
        format!("{}.{key}", self.id)
    }

    /// What keeps its settings from being shown: its contract's problems,
    /// and a key whose setting id egui-pop refuses.
    pub fn problems(&self) -> Vec<String> {
        let mut p = self.contract.problems();
        for name in self.contract.keys.keys() {
            if let Err(e) = SettingId::new(self.setting(name)) {
                p.push(format!("setting `{name}`: {e:?}"));
            }
        }
        p
    }

    /// The keyring account its secrets are kept under.
    fn account(&self) -> String {
        format!("plugin:{}", self.id)
    }

    /// The slug of its section's path.
    fn slug(&self) -> String {
        self.id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }
}

/// A key's label and help, through the plugin's catalogue
/// (`setting-<plugin>-<key>` with `.label` and `.help`), else as the
/// contract writes them.
fn texts(
    p: &PluginSettings,
    name: &str,
    key: &ConfigKey,
    t: &dyn Fn(Message) -> String,
) -> (String, String) {
    let msg = helios_core::config::message(&p.id, Level::Own, name);
    let label = if key.label.is_empty() {
        name
    } else {
        &key.label
    };
    let label = t(Message::new(&format!("{msg}.label"), label));
    let help = if key.help.is_empty() {
        String::new()
    } else {
        t(Message::new(&format!("{msg}.help"), &key.help))
    };
    (label, help)
}

/// A contract key's egui-pop descriptor. Its default is the contract's,
/// else the type's (false, the lowest number allowed, empty, the first
/// choice). The contract is sound ([`PluginSettings::problems`]).
pub fn descriptor(key: &ConfigKey, label: &str, help: &str) -> Descriptor {
    let default = key.default.as_ref();
    // A number without bounds still gets a range a control can span.
    let (lo, hi) = (key.min.unwrap_or(-1e9), key.max.unwrap_or(1e9));
    let base = lo.max(0.0).min(hi);
    let d = match key.ty {
        KeyType::Bool => Descriptor::bool(label, default.and_then(Json::as_bool).unwrap_or(false)),
        KeyType::Int => {
            let d = Descriptor::int(
                label,
                lo as i64,
                hi as i64,
                default.and_then(Json::as_i64).unwrap_or(base as i64),
            );
            match key.step {
                Some(s) => d.step(s),
                None => d,
            }
        }
        KeyType::Float => {
            let d = Descriptor::float(
                label,
                lo,
                hi,
                default.and_then(Json::as_f64).unwrap_or(base),
            );
            match key.step {
                Some(s) => d.step(s),
                None => d,
            }
        }
        KeyType::String => {
            Descriptor::text(label, default.and_then(Json::as_str).unwrap_or_default())
                .capacity(4096)
        }
        KeyType::Choice => {
            let choices: Vec<&str> = key.choices.iter().map(String::as_str).collect();
            let first = choices.first().copied().unwrap_or_default();
            Descriptor::choice(
                label,
                &choices,
                default.and_then(Json::as_str).unwrap_or(first),
            )
        }
        KeyType::Path => {
            let kind = match key.path {
                Some(PathKind::File) => PopPath::File,
                _ => PopPath::Dir,
            };
            let mut d = Descriptor::path(label, kind, &[]);
            if let Some(text) = default.and_then(Json::as_str) {
                d.default = egui_pop::model::values::Value::Text(text.to_string());
            }
            d
        }
        KeyType::Secret => Descriptor::text(label, "").capacity(4096).sensitive(),
    };
    d.description(help)
}

/// Declares every sound plugin's settings; a plugin whose contract has
/// problems declares none (its section says why).
pub fn declare(
    ctx: &mut egui_pop::api::Ctx<'_>,
    plugins: &[PluginSettings],
    t: &dyn Fn(Message) -> String,
) {
    for p in plugins.iter().filter(|p| p.problems().is_empty()) {
        for (name, key) in p.contract.ordered() {
            let (label, help) = texts(p, name, key, t);
            let Ok(id) = SettingId::new(p.setting(name)) else {
                continue;
            };
            if let Err(e) = ctx.setting(id, descriptor(key, &label, &help)) {
                log::warn!("plugin `{}`: setting `{name}` not declared: {e:?}", p.id);
            }
        }
    }
}

/// A field's id: its setting id as a `when` clause can name it (a
/// plugin id's `-` is not a clause's).
fn field_id(setting: &str) -> String {
    setting.replace('-', "_")
}

/// The `when` clause of a key's `visible_when`: each key named has the
/// value given (one of them, for a list), read from the section's values.
fn clause(p: &PluginSettings, key: &ConfigKey) -> Option<String> {
    let quote = |v: &Json| {
        let text = egui_pop::api::admin::model::cell_text(v);
        format!("'{}'", text.replace('\'', ""))
    };
    let parts: Vec<String> = key
        .visible_when
        .iter()
        .map(|(other, wanted)| {
            let at = format!("form.{}", field_id(&p.setting(other)));
            let any: Vec<String> = match wanted {
                Json::Array(many) => many
                    .iter()
                    .map(|v| format!("{at} == {}", quote(v)))
                    .collect(),
                one => vec![format!("{at} == {}", quote(one))],
            };
            format!("({})", any.join(" || "))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" && "))
}

/// The manifest's sections of the Plugins scope: one per plugin, its
/// permissions (`plugin.capabilities` of that plugin, each with a switch)
/// and its settings from the contract, in its groups; its problems
/// instead when the contract has any.
pub fn sections(
    m: egui_pop::api::admin::model::Manifest,
    plugins: &[PluginSettings],
    t: &dyn Fn(Message) -> String,
) -> egui_pop::api::admin::model::Manifest {
    let mut m = m;
    for p in plugins {
        let title = t(Message::new(
            &helios_i18n::plugins::title(&p.id).key(),
            &p.title,
        ));
        m = m.section(&format!("plugins/{}", p.slug()), &title, |s| {
            section(s, p, t);
        });
    }
    m
}

/// One plugin's section.
pub fn section(s: &mut SectionBuilder<'_>, p: &PluginSettings, t: &dyn Fn(Message) -> String) {
    if !p.description.is_empty() {
        s.description(&t(Message::new(
            &helios_i18n::plugins::description(&p.id).key(),
            &p.description,
        )));
    }
    permissions(s, p, t);
    let problems = p.problems();
    if !problems.is_empty() {
        s.callout(Callout::new(
            "plugin-settings-refused",
            Tone::Danger,
            &t(Message::new(
                "gui-settings-plugin-refused",
                "This plugin's settings are not shown: { $problems }",
            )
            .with("problems", problems.join("; "))),
        ));
        return;
    }
    let mut group: Option<&str> = None;
    for (name, key) in p.contract.ordered() {
        if group != Some(key.group.as_str()) {
            group = Some(&key.group);
            let id = if key.group.is_empty() {
                "settings".to_string()
            } else {
                format!("group-{}", key.group.to_lowercase().replace(' ', "-"))
            };
            let heading = if key.group.is_empty() {
                t(Message::new("gui-settings-plugin-settings", "Settings"))
            } else {
                key.group.clone()
            };
            s.group(&id, &heading);
        }
        let mut f = Field::setting(&p.setting(name));
        // Its id is how the section's clauses name its value.
        f.meta.id = field_id(&p.setting(name));
        if key.per_library {
            f = f.scopes(&[LIBRARY.scope]);
        }
        if let Some(c) = clause(p, key) {
            f = f.when(&c);
        }
        s.field(f);
    }
}

/// A plugin's capabilities, each with the reason its manifest gives and
/// a switch granting or taking it back.
fn permissions(s: &mut SectionBuilder<'_>, p: &PluginSettings, t: &dyn Fn(Message) -> String) {
    use helios_plugin::commands::{CAPABILITIES, GRANT};
    s.group(
        "permissions",
        &t(Message::new(
            "gui-settings-plugins-capabilities",
            "Permissions",
        )),
    );
    s.table(
        Records::command(
            "plugin-capabilities",
            "",
            CAPABILITIES,
            "/capabilities",
            "id",
        )
        .args(json!({ "plugin": p.id }))
        .column(
            "reason",
            &t(Message::new("gui-settings-plugin-reason", "Why")),
        )
        .row_field(RowField::toggle(
            "granted",
            &t(Message::new("gui-settings-plugin-granted", "Allowed")),
            GRANT,
            json!({
                "plugin": "${row.plugin}",
                "capability": "${row.capability}",
                "granted": "${value}",
            }),
        ))
        .empty(
            &t(Message::new(
                "gui-settings-plugin-no-capability",
                "It asks for no permission.",
            )),
            None,
        ),
    );
}

/// Where the plugins' settings are kept: the app's values in a file,
/// a library's in the node's layer for it, secrets in the keyring. Clones
/// share everything.
#[derive(Clone)]
pub struct PluginSettingsStore {
    inner: Arc<Inner>,
}

struct Inner {
    /// The sound plugins.
    plugins: Vec<PluginSettings>,
    /// Setting id → (plugin, key).
    keys: BTreeMap<String, (usize, String)>,
    file: PathBuf,
    /// Helios's settings, read again after a library write (they keep
    /// the same layer).
    helios: NodeSettingsStore,
    node: Option<Arc<dyn NodeLayers>>,
    /// The library's layer name, when one is open.
    project: Option<String>,
    secrets: Arc<dyn Secrets>,
    watchers: Mutex<BTreeMap<ValueScope, OnChange>>,
    /// The library's values as last read or written.
    library: Mutex<Values>,
    /// What reading each scope said: values refused, with their reason.
    refused: Mutex<BTreeMap<ValueScope, Vec<String>>>,
    /// One write at a time.
    writing: Mutex<()>,
}

impl std::fmt::Debug for PluginSettingsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginSettingsStore")
            .field("file", &self.inner.file)
            .finish_non_exhaustive()
    }
}

/// A TOML value as JSON.
fn to_json(v: &toml::Value) -> Json {
    serde_json::to_value(v).unwrap_or(Json::Null)
}

impl PluginSettingsStore {
    /// The sound plugins of `plugins`, their app values in
    /// [`PLUGINS_FILE`] in `config_dir`, their library values in the
    /// layer `helios` keeps for the open library, their secrets in
    /// `secrets`.
    pub fn new(
        plugins: &[PluginSettings],
        helios: &NodeSettingsStore,
        config_dir: &Path,
        secrets: Arc<dyn Secrets>,
    ) -> PluginSettingsStore {
        let plugins: Vec<PluginSettings> = plugins
            .iter()
            .filter(|p| p.problems().is_empty())
            .cloned()
            .collect();
        let mut keys = BTreeMap::new();
        for (i, p) in plugins.iter().enumerate() {
            for name in p.contract.keys.keys() {
                keys.insert(p.setting(name), (i, name.clone()));
            }
        }
        let store = PluginSettingsStore {
            inner: Arc::new(Inner {
                plugins,
                keys,
                file: config_dir.join(PLUGINS_FILE),
                helios: helios.clone(),
                node: helios.node(),
                project: helios.settings().project().map(String::from),
                secrets,
                watchers: Mutex::new(BTreeMap::new()),
                library: Mutex::new(Values::new()),
                refused: Mutex::new(BTreeMap::new()),
                writing: Mutex::new(()),
            }),
        };
        let me = store.clone();
        helios.also_follow(Arc::new(move |name: &str| me.layer_changed(name)));
        if let Ok(values) = store.read_library() {
            *lock(&store.inner.library) = values;
        }
        store
    }

    fn key(&self, id: &str) -> Option<(&PluginSettings, &str, &ConfigKey)> {
        let (i, name) = self.inner.keys.get(id)?;
        let p = &self.inner.plugins[*i];
        Some((p, name, p.contract.get(name)?))
    }

    /// Values read for a scope, by plugin then key, checked: the ones
    /// the contract refuses are left out and said in
    /// [`SettingsStore::problems`].
    fn checked(&self, by_plugin: &Map<String, Json>, scope: ValueScope) -> Values {
        let mut out = Values::new();
        let mut refused = Vec::new();
        for (plugin, values) in by_plugin {
            let Some(values) = values.as_object() else {
                refused.push(format!("plugin `{plugin}`: its values are not a table"));
                continue;
            };
            for (name, v) in values {
                let id = format!("{plugin}.{name}");
                let Some((_, _, key)) = self.key(&id) else {
                    refused.push(format!("`{id}`: no plugin installed declares it"));
                    continue;
                };
                let why = if key.ty == KeyType::Secret {
                    Some("a secret is kept in the keyring, never in a file".to_string())
                } else if scope != ValueScope::App && !key.per_library {
                    Some("not set per library".to_string())
                } else {
                    key.check(v).err()
                };
                match why {
                    Some(why) => refused.push(format!("`{id}`: {why}; left out")),
                    None => {
                        out.insert(id, v.clone());
                    }
                }
            }
        }
        for r in &refused {
            log::warn!("the plugins' settings ({}): {r}", scope.name());
        }
        lock(&self.inner.refused).insert(scope, refused);
        out
    }

    fn read_file(&self) -> Result<Map<String, Json>, String> {
        let at = |e: &dyn std::fmt::Display| format!("{}: {e}", self.inner.file.display());
        match std::fs::read_to_string(&self.inner.file) {
            Ok(text) if text.trim().is_empty() => Ok(Map::new()),
            Ok(text) => serde_json::from_str(&text).map_err(|e| at(&e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
            Err(e) => Err(at(&e)),
        }
    }

    fn write_file(&self, values: &Map<String, Json>) -> Result<(), String> {
        let file = &self.inner.file;
        let at = |e: std::io::Error| format!("{}: {e}", file.display());
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(at)?;
        }
        let text = serde_json::to_string_pretty(values).map_err(|e| e.to_string())?;
        let mut tmp = file.as_os_str().to_owned();
        tmp.push(".tmp");
        std::fs::write(&tmp, text + "\n").map_err(at)?;
        std::fs::rename(&tmp, file).map_err(at)
    }

    /// The library's layer as a TOML table.
    fn library_layer(&self) -> Result<Option<(String, toml::Table)>, String> {
        let (Some(node), Some(name)) = (&self.inner.node, &self.inner.project) else {
            return Ok(None);
        };
        let text = node.read(name)?.unwrap_or_default();
        let table: toml::Table = text
            .parse()
            .map_err(|e| format!("the library's settings: {e}"))?;
        Ok(Some((name.clone(), table)))
    }

    fn read_library(&self) -> Result<Values, String> {
        let Some((_, table)) = self.library_layer()? else {
            return Ok(Values::new());
        };
        let by_plugin = match table.get(LIBRARY_TABLE).map(to_json) {
            Some(Json::Object(m)) => m,
            _ => Map::new(),
        };
        Ok(self.checked(&by_plugin, LIBRARY.scope))
    }

    /// The app's values, the secrets set shown as [`SECRET_SET`].
    fn read_app(&self) -> Result<Values, String> {
        let mut out = self.checked(&self.read_file()?, ValueScope::App);
        for p in &self.inner.plugins {
            for name in p.contract.secrets() {
                match self.inner.secrets.get(&p.account(), name) {
                    Ok(Some(_)) => {
                        out.insert(p.setting(name), json!(SECRET_SET));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::warn!("plugin `{}`: secret `{name}` not read: {e}", p.id);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Keeps a secret: empty or taken back forgets it, [`SECRET_SET`]
    /// leaves it as it is. Its value is never in a message.
    fn save_secret(
        &self,
        p: &PluginSettings,
        name: &str,
        value: Option<&Json>,
    ) -> Result<(), String> {
        let text = value.and_then(Json::as_str).unwrap_or_default();
        let at = |e: String| format!("`{}`: {e}", p.setting(name));
        if text == SECRET_SET {
            return Ok(());
        }
        // Typed after the mask: what was typed.
        let text = text.strip_prefix(SECRET_SET).unwrap_or(text);
        if text.is_empty() {
            self.inner.secrets.delete(&p.account(), name).map_err(at)
        } else {
            self.inner.secrets.set(&p.account(), name, text).map_err(at)
        }
    }

    fn save_app(&self, changes: &Changes) -> Result<(), String> {
        let mut values = self.read_file()?;
        let mut errors = Vec::new();
        let mut touched = false;
        for (id, value) in changes {
            let Some((p, name, key)) = self.key(id) else {
                errors.push(format!("`{id}`: no plugin installed declares it"));
                continue;
            };
            if key.ty == KeyType::Secret {
                if let Err(e) = self.save_secret(p, name, value.as_ref()) {
                    errors.push(e);
                }
                continue;
            }
            if let Some(v) = value
                && let Err(e) = key.check(v)
            {
                errors.push(format!("`{id}`: {e}"));
                continue;
            }
            let table = values
                .entry(p.id.clone())
                .or_insert_with(|| Json::Object(Map::new()));
            if let Some(t) = table.as_object_mut() {
                match value {
                    Some(v) => t.insert(name.to_string(), v.clone()),
                    None => t.remove(name),
                };
                touched = true;
            }
        }
        values.retain(|_, t| t.as_object().is_none_or(|o| !o.is_empty()));
        if touched && let Err(e) = self.write_file(&values) {
            errors.push(e);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    fn save_library(&self, changes: &Changes) -> Result<(), String> {
        let Some((layer, mut table)) = self.library_layer()? else {
            return Err("no library is open".into());
        };
        let mut errors = Vec::new();
        let plugins = table
            .entry(LIBRARY_TABLE)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(plugins) = plugins.as_table_mut() else {
            return Err(format!("the library's `{LIBRARY_TABLE}` is not a table"));
        };
        for (id, value) in changes {
            let Some((p, name, key)) = self.key(id) else {
                errors.push(format!("`{id}`: no plugin installed declares it"));
                continue;
            };
            if !key.per_library {
                errors.push(format!("`{id}` is not set per library"));
                continue;
            }
            let entry = plugins
                .entry(p.id.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            let Some(entry) = entry.as_table_mut() else {
                continue;
            };
            match value {
                Some(v) => match key
                    .check(v)
                    .and_then(|()| toml::Value::try_from(v).map_err(|e| e.to_string()))
                {
                    Ok(t) => {
                        entry.insert(name.to_string(), t);
                    }
                    Err(e) => errors.push(format!("`{id}`: {e}")),
                },
                None => {
                    entry.remove(name);
                }
            }
        }
        plugins.retain(|_, t| t.as_table().is_none_or(|t| !t.is_empty()));
        if plugins.is_empty() {
            table.remove(LIBRARY_TABLE);
        }
        let text = toml::to_string(&table).map_err(|e| e.to_string())?;
        if let Some(node) = &self.inner.node {
            node.write(&layer, &text)?;
        }
        // Helios's settings keep the same layer: they read it again now,
        // so their next write keeps these values.
        self.inner.helios.layer_changed(&layer);
        if let Ok(values) = self.read_library() {
            *lock(&self.inner.library) = values;
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// A layer of the node changed: the library's is read again and what
    /// moved told to the watcher.
    fn layer_changed(&self, name: &str) {
        if self.inner.project.as_deref() != Some(name) {
            return;
        }
        let changes = {
            let _one = lock(&self.inner.writing);
            let Ok(after) = self.read_library() else {
                return;
            };
            let mut before = lock(&self.inner.library);
            let changes = diff(&before, &after);
            *before = after;
            changes
        };
        if changes.is_empty() {
            return;
        }
        let watcher = lock(&self.inner.watchers).get(&LIBRARY.scope).cloned();
        if let Some(w) = watcher {
            w(LIBRARY.scope, changes);
        }
    }
}

impl SettingsStore for PluginSettingsStore {
    fn scopes(&self) -> Vec<ValueScope> {
        let mut out = vec![ValueScope::App];
        if self.inner.project.is_some() {
            out.push(LIBRARY.scope);
        }
        out
    }

    fn claims(&self, setting: &str) -> bool {
        self.inner.keys.contains_key(setting)
    }

    fn load(&self, scope: ValueScope) -> Result<Values, String> {
        let _one = lock(&self.inner.writing);
        if scope == ValueScope::App {
            self.read_app()
        } else if scope == LIBRARY.scope {
            let values = self.read_library()?;
            lock(&self.inner.library).clone_from(&values);
            Ok(values)
        } else {
            Err(format!("plugins keep no {} settings", scope.name()))
        }
    }

    fn save(&self, scope: ValueScope, changes: &Changes) -> Result<(), String> {
        let _one = lock(&self.inner.writing);
        if scope == ValueScope::App {
            self.save_app(changes)
        } else if scope == LIBRARY.scope {
            self.save_library(changes)
        } else {
            Err(format!("plugins keep no {} settings", scope.name()))
        }
    }

    fn watch(&self, scope: ValueScope, on_change: OnChange) -> Result<(), String> {
        lock(&self.inner.watchers).insert(scope, on_change);
        Ok(())
    }

    fn problems(&self) -> Vec<String> {
        lock(&self.inner.refused)
            .values()
            .flatten()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Settings, Sources};
    use egui_pop::api::admin::model::Item;
    use egui_pop::model::values::Type;
    use helios_connect::account::MemorySecrets;

    const ID: &str = "org.example.fake-one";
    const PROJECT: &str = "project//photos";

    fn plugin(contract: &str) -> PluginSettings {
        PluginSettings {
            id: ID.into(),
            title: "Fake".into(),
            description: "A fake plugin".into(),
            contract: serde_json::from_str(contract).unwrap(),
        }
    }

    fn fake() -> PluginSettings {
        plugin(
            r#"{
                "quality": { "type": "int", "default": 90, "min": 1, "max": 100, "step": 5,
                             "label": "Quality", "per_library": true },
                "mode": { "type": "choice", "choices": ["fast", "best"], "default": "fast",
                          "label": "Mode", "group": "Speed" },
                "ratio": { "type": "float", "min": 0, "max": 2 },
                "on": { "type": "bool", "default": true },
                "into": { "type": "path", "path": "folder" },
                "note": { "type": "string", "visible_when": { "mode": ["best"] } },
                "token": { "type": "secret", "label": "Token" }
            }"#,
        )
    }

    /// The node's layers in memory.
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

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("helios-plugins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn store(
        dir: &Path,
        layers: &Arc<Layers>,
        secrets: &Arc<MemorySecrets>,
    ) -> (NodeSettingsStore, PluginSettingsStore) {
        let settings = Settings::resolve(&Sources {
            project: Some((PROJECT.into(), None)),
            ..Sources::default()
        });
        let node =
            NodeSettingsStore::new(settings, Some(layers.clone() as Arc<dyn NodeLayers>), dir);
        let p = PluginSettingsStore::new(&[fake()], &node, dir, secrets.clone());
        (node, p)
    }

    #[test]
    fn a_contract_maps_to_descriptors_and_fields() {
        let p = fake();
        assert!(p.problems().is_empty(), "{:?}", p.problems());
        let d = |name: &str| descriptor(p.contract.get(name).unwrap(), name, "help");
        let quality = d("quality");
        assert_eq!(quality.ty, Type::Int);
        assert_eq!(quality.hard, Some((1.0, 100.0)));
        assert_eq!(quality.step, 5.0);
        assert_eq!(quality.default.to_json(), json!(90));
        let mode = d("mode");
        assert_eq!(mode.ty, Type::Choice);
        assert_eq!(mode.choices, ["fast", "best"]);
        assert_eq!(d("ratio").ty, Type::Float);
        assert_eq!(d("on").default.to_json(), json!(true));
        assert!(d("into").path.is_some());
        let token = d("token");
        assert!(token.sensitive, "a secret is sensitive");
        assert_eq!(token.default.to_json(), json!(""));

        let m = crate::config::manifest(&helios_config::Schema::core(), &[p], &|m| m.english);
        let section = m
            .sections
            .iter()
            .find(|s| s.scope == "plugins" && s.meta.id == "org-example-fake-one")
            .expect("the plugin's section");
        assert_eq!(section.meta.title, "Fake");
        let fields: Vec<_> = section
            .groups
            .iter()
            .flat_map(|g| &g.items)
            .filter_map(|i| match i {
                Item::Field(f) => Some(f),
                _ => None,
            })
            .collect();
        let ids: Vec<&str> = fields.iter().filter_map(|f| f.setting_id()).collect();
        assert_eq!(ids.len(), 7, "{ids:?}");
        let field = |id: &str| {
            fields
                .iter()
                .find(|f| f.setting_id() == Some(&format!("{ID}.{id}")))
                .unwrap()
        };
        assert_eq!(field("quality").scopes, [LIBRARY.scope]);
        assert!(field("mode").scopes.is_empty());
        assert_eq!(
            field("note").meta.when.as_deref(),
            Some("(form.org.example.fake_one.mode == 'best')")
        );
        assert!(
            section.groups.iter().any(|g| g.meta.title == "Speed"),
            "the contract's groups"
        );
        assert!(
            section
                .groups
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| matches!(i, Item::Table(_))),
            "its permissions"
        );
    }

    #[test]
    fn an_invalid_default_is_reported_not_crashed_on() {
        let bad = plugin(r#"{ "quality": { "type": "int", "default": "high" } }"#);
        let problems = bad.problems();
        assert!(
            problems.iter().any(|p| p.contains("default")),
            "{problems:?}"
        );
        let m = crate::config::manifest(
            &helios_config::Schema::core(),
            std::slice::from_ref(&bad),
            &|m| m.english,
        );
        let section = m
            .sections
            .iter()
            .find(|s| s.meta.id == "org-example-fake-one")
            .unwrap();
        let items: Vec<&Item> = section.groups.iter().flat_map(|g| &g.items).collect();
        assert!(
            !items.iter().any(|i| matches!(i, Item::Field(_))),
            "no field"
        );
        let said = items
            .iter()
            .find_map(|i| match i {
                Item::Callout(c) => Some(c.text.clone()),
                _ => None,
            })
            .expect("a callout says why");
        assert!(said.contains("default"), "{said}");
        // Its store claims nothing.
        let dir = temp("bad");
        let node = NodeSettingsStore::new(Settings::default(), None, &dir);
        let s = PluginSettingsStore::new(&[bad], &node, &dir, Arc::new(MemorySecrets::default()));
        assert!(!s.claims(&format!("{ID}.quality")));
    }

    #[test]
    fn values_save_to_their_place_and_bad_ones_are_refused_with_a_reason() {
        let dir = temp("save");
        let layers = Arc::new(Layers::default());
        let secrets = Arc::new(MemorySecrets::default());
        let (node, s) = store(&dir, &layers, &secrets);
        let id = |k: &str| format!("{ID}.{k}");
        let one = |k: &str, v: Json| -> Changes { [(id(k), Some(v))].into_iter().collect() };

        // The app's value lands in its file.
        s.save(ValueScope::App, &one("mode", json!("best")))
            .unwrap();
        let file: Json =
            serde_json::from_str(&std::fs::read_to_string(dir.join(PLUGINS_FILE)).unwrap())
                .unwrap();
        assert_eq!(file[ID]["mode"], "best");
        assert_eq!(s.load(ValueScope::App).unwrap()[&id("mode")], json!("best"));

        // The library's value reaches the node, beside Helios's own.
        s.save(LIBRARY.scope, &one("quality", json!(40))).unwrap();
        let text = layers.read(PROJECT).unwrap().unwrap();
        assert!(
            text.contains("[plugin-settings.\"org.example.fake-one\"]"),
            "{text}"
        );
        assert!(text.contains("quality = 40"), "{text}");
        assert_eq!(s.load(LIBRARY.scope).unwrap()[&id("quality")], json!(40));
        // Helios writing its library layer keeps it.
        node.save(
            LIBRARY.scope,
            &[("project.decisions.mode".to_string(), Some(json!("human")))]
                .into_iter()
                .collect(),
        )
        .unwrap();
        let text = layers.read(PROJECT).unwrap().unwrap();
        assert!(
            text.contains("quality = 40") && text.contains("human"),
            "{text}"
        );

        // Refused, with the reason.
        let e = s
            .save(ValueScope::App, &one("quality", json!(500)))
            .unwrap_err();
        assert!(e.contains("500 is above 100"), "{e}");
        let e = s
            .save(LIBRARY.scope, &one("mode", json!("fast")))
            .unwrap_err();
        assert!(e.contains("not set per library"), "{e}");
        let e = s.save(ValueScope::App, &one("nope", json!(1))).unwrap_err();
        assert!(e.contains("no plugin installed declares it"), "{e}");

        // Values the contract refuses, read from the file, are left out
        // and said.
        std::fs::write(
            dir.join(PLUGINS_FILE),
            json!({ ID: { "quality": "high", "nope": 1, "on": false } }).to_string(),
        )
        .unwrap();
        let values = s.load(ValueScope::App).unwrap();
        assert_eq!(values.get(&id("on")), Some(&json!(false)));
        assert!(!values.contains_key(&id("quality")));
        let problems = s.problems().join("\n");
        assert!(
            problems.contains("`org.example.fake-one.quality`: expected integer"),
            "{problems}"
        );
        assert!(problems.contains("`org.example.fake-one.nope`: no plugin installed declares it"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_goes_to_the_keyring_and_never_reads_back() {
        let dir = temp("secret");
        let layers = Arc::new(Layers::default());
        let secrets = Arc::new(MemorySecrets::default());
        let (_node, s) = store(&dir, &layers, &secrets);
        let token = format!("{ID}.token");
        let set = |v: Option<Json>| -> Changes { [(token.clone(), v)].into_iter().collect() };
        s.save(ValueScope::App, &set(Some(json!("hunter2-secret"))))
            .unwrap();
        assert_eq!(
            secrets
                .get("plugin:org.example.fake-one", "token")
                .unwrap()
                .as_deref(),
            Some("hunter2-secret")
        );
        assert!(!dir.join(PLUGINS_FILE).exists(), "never in the file");
        // It loads as the mask, which saved back changes nothing.
        let loaded = s.load(ValueScope::App).unwrap();
        assert_eq!(loaded[&token], json!(SECRET_SET));
        s.save(ValueScope::App, &set(Some(json!(SECRET_SET))))
            .unwrap();
        assert!(
            secrets
                .get("plugin:org.example.fake-one", "token")
                .unwrap()
                .is_some()
        );
        // Taken back, it is forgotten.
        s.save(ValueScope::App, &set(None)).unwrap();
        assert!(
            secrets
                .get("plugin:org.example.fake-one", "token")
                .unwrap()
                .is_none()
        );
        // A secret left in the file is refused, without its value.
        std::fs::write(
            dir.join(PLUGINS_FILE),
            json!({ ID: { "token": "leaked-value" } }).to_string(),
        )
        .unwrap();
        assert!(!s.load(ValueScope::App).unwrap().contains_key(&token));
        let problems = s.problems().join("\n");
        assert!(problems.contains("never in a file"), "{problems}");
        assert!(!problems.contains("leaked-value"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_library_value_another_client_changes_is_told_to_the_watcher() {
        let dir = temp("watch");
        let layers = Arc::new(Layers::default());
        let secrets = Arc::new(MemorySecrets::default());
        let (_node, s) = store(&dir, &layers, &secrets);
        let heard: Arc<Mutex<Vec<Changes>>> = Arc::default();
        let h = heard.clone();
        s.watch(LIBRARY.scope, Arc::new(move |_, c| lock(&h).push(c)))
            .unwrap();
        // This store's own write is no change.
        s.save(
            LIBRARY.scope,
            &[(format!("{ID}.quality"), Some(json!(30)))]
                .into_iter()
                .collect(),
        )
        .unwrap();
        s.layer_changed(PROJECT);
        assert!(lock(&heard).is_empty());
        // Another client's is.
        layers
            .write(
                PROJECT,
                "[plugin-settings.\"org.example.fake-one\"]\nquality = 55\n",
            )
            .unwrap();
        s.layer_changed(PROJECT);
        let heard = lock(&heard).clone();
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0][&format!("{ID}.quality")], Some(json!(55)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
