//! Helios's own settings (`helios-config`) for the desktop: resolved once
//! from their layers, read by every part through [`Settings`], and shown
//! in the Settings app ([`Bridge`]).
//!
//! **Layers, as `helios-config` says** ([`helios_config::Concern::layers`]):
//!
//! | layer | where the desktop keeps it |
//! |---|---|
//! | `global` | this machine: `<config>/global.toml` ([`GLOBAL_FILE`]) |
//! | `project` | the library: the vault's `project/<library folder>` layer |
//! | `user` | the person, on every machine: the vault's `user` layer |
//! | `run` | the command line's `--set KEY=VALUE` |
//!
//! A value an older program left in a layer its concern refuses (a
//! machine setting in the vault's `user` layer) is read as `global`
//! ([`helios_config::Config::load_str_lenient`]) and moves to the
//! machine's file at the next save.
//!
//! **In the Settings app.** Each setting is declared as an egui-pop setting
//! of the same id and placed by a manifest in its scope and section. Its
//! `app` value is the layer the desktop edits for its concern (`global`
//! for machine and project settings, `user` for the person's), its
//! `library` value the project layer (project settings only), and
//! `--set` its `command line` origin. Edits are written back to their
//! layer on the next pass; they take effect at the next start.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::admin::model::{FORMAT, Field, Manifest, ValueScope};
use egui_pop::api::admin::{ADMIN, Admin};
use egui_pop::api::hooks::On;
use egui_pop::api::services::ServiceToken;
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::Origin;
use egui_pop::model::ids::{OwnerId, PluginId, SettingId};
use egui_pop::model::values::{Change, Descriptor, Set, Value as PopValue};
use helios_config::{Concern, Config, Kind, Layer, Schema, Value};
use serde_json::{Value as Json, json};

/// The machine's layer, in the configuration folder.
pub const GLOBAL_FILE: &str = "global.toml";

/// The vault's name for the person's layer.
pub const USER_LAYER: &str = "user";

/// The vault's name for a library's project layer: `project/<folder>`,
/// the folder holding its catalogue (as `helios-node --project` names a
/// project by its folder).
pub fn project_layer(vault_file: &Path) -> Option<String> {
    let file = std::path::absolute(vault_file).unwrap_or_else(|_| vault_file.to_path_buf());
    let dir = file.parent()?;
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    Some(format!("project/{}", dir.display()))
}

/// The layers' texts, as the program's edge read them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    /// The machine's file's text.
    pub global: Option<String>,
    /// The library's project layer: its vault name and its text.
    pub project: Option<(String, Option<String>)>,
    /// The vault's `user` layer.
    pub user: Option<String>,
    /// `--set KEY=VALUE`.
    pub run: Vec<(String, String)>,
}

/// The settings resolved through their layers, and what reading them said.
#[derive(Clone, Debug)]
pub struct Settings {
    config: Config,
    /// The project layer's vault name, when a library is open.
    project: Option<String>,
    problems: Vec<String>,
    /// Each editable layer's text as last read or written.
    saved: BTreeMap<Layer, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::resolve(&Sources::default())
    }
}

impl Settings {
    /// Reads every layer, lowest first; what does not read is left out
    /// and said in [`Settings::problems`], never a failure.
    pub fn resolve(sources: &Sources) -> Settings {
        let mut config = Config::new(Schema::core());
        let mut problems = Vec::new();
        let mut load = |config: &mut Config, layer: Layer, text: Option<&str>, name: &str| {
            let Some(text) = text else { return };
            let l = config.load_str_lenient(layer, text);
            for key in l.moved {
                problems.push(format!(
                    "{name}: `{key}` is not set in the {} layer; read as this machine's",
                    layer.name()
                ));
            }
            problems.extend(l.problems.iter().map(|p| format!("{name}: {p}")));
        };
        load(
            &mut config,
            Layer::Global,
            sources.global.as_deref(),
            GLOBAL_FILE,
        );
        if let Some((name, text)) = &sources.project {
            load(&mut config, Layer::Project, text.as_deref(), name);
        }
        load(
            &mut config,
            Layer::User,
            sources.user.as_deref(),
            USER_LAYER,
        );
        for (key, value) in &sources.run {
            // The run layer names any setting; only the ones known matter.
            if config.schema().get(key).is_some()
                && let Err(p) = config.set_str(Layer::Run, key, value)
            {
                problems.push(format!("--set {key}={value}: {p}"));
            }
        }
        let mut saved = BTreeMap::new();
        saved.insert(Layer::Global, normal(sources.global.as_deref()));
        saved.insert(Layer::User, normal(sources.user.as_deref()));
        if let Some((_, text)) = &sources.project {
            saved.insert(Layer::Project, normal(text.as_deref()));
        }
        Settings {
            config,
            project: sources.project.as_ref().map(|(n, _)| n.clone()),
            problems,
            saved,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// What reading the layers said (values left out or moved).
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// The project layer's vault name, when a library is open.
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// A setting counted in MiB, in bytes (at least 1 MiB).
    pub fn mib(&self, key: &str) -> u64 {
        (self.config.number(key).max(1.0) as u64) * 1024 * 1024
    }

    /// The layer the desktop edits a setting in as its `app` value: the
    /// machine's for machine and project settings, the person's for
    /// theirs.
    pub fn home(concern: Concern) -> Layer {
        match concern {
            Concern::Machine | Concern::Project => Layer::Global,
            Concern::User => Layer::User,
        }
    }

    /// The `app` value of a setting: its home layer's, else the machine's,
    /// else the default.
    fn app_value(&self, key: &str) -> Option<Value> {
        let s = self.config.schema().get(key)?;
        let home = Settings::home(s.concern);
        Some(
            self.config
                .value_in(home, key)
                .or_else(|| self.config.value_in(Layer::Global, key))
                .unwrap_or_else(|| s.kind.default_value()),
        )
    }

    /// Sets (`Some`) or takes back (`None`) a value of a layer.
    ///
    /// # Errors
    /// The layer does not take it.
    pub fn set(&mut self, layer: Layer, key: &str, value: Option<Value>) -> Result<(), String> {
        if let Some(v) = value {
            return self.config.set(layer, key, v).map_err(|p| p.to_string());
        }
        self.config.unset(layer, key);
        Ok(())
    }

    /// The editable layers whose values changed since they were read or
    /// last taken, with their new text; they count as saved from now on.
    pub fn take_changed(&mut self) -> Vec<(Layer, String)> {
        let mut out = Vec::new();
        for layer in [Layer::Global, Layer::Project, Layer::User] {
            if layer == Layer::Project && self.project.is_none() {
                continue;
            }
            let text = self.config.layer_toml(layer);
            if self.saved.get(&layer) != Some(&text) {
                self.saved.insert(layer, text.clone());
                out.push((layer, text));
            }
        }
        out
    }
}

/// A layer's text as compared: what its values give, whatever the
/// spacing or order written.
fn normal(text: Option<&str>) -> String {
    let mut c = Config::new(Schema::core());
    let _ = c.load_str_lenient(Layer::Global, text.unwrap_or(""));
    c.layer_toml(Layer::Global)
}

/// Reads the layers: the machine's file in `config_dir`, the vault's
/// `user` layer and the library's project layer (`project`: its vault
/// name), `--set`.
pub fn read(
    config_dir: &Path,
    vault_layer: &dyn Fn(&str) -> Result<Option<String>, String>,
    project: Option<String>,
    run: &[(String, String)],
) -> (Sources, Vec<String>) {
    let mut problems = Vec::new();
    let file = config_dir.join(GLOBAL_FILE);
    let global = match std::fs::read_to_string(&file) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            problems.push(format!("cannot read {}: {e}", file.display()));
            None
        }
    };
    let mut layer = |name: &str| {
        vault_layer(name).unwrap_or_else(|e| {
            problems.push(format!("the vault's `{name}` settings: {e}"));
            None
        })
    };
    let user = layer(USER_LAYER);
    let project = project.map(|name| {
        let text = layer(&name);
        (name, text)
    });
    (
        Sources {
            global,
            project,
            user,
            run: run.to_vec(),
        },
        problems,
    )
}

/// Writes a vault layer (its name and text).
pub type VaultWriter = Box<dyn Fn(&str, &str) -> Result<(), String> + Send>;

/// The Settings app's side: the settings as egui-pop settings, their
/// manifest, and the values written back to their layers.
pub const BRIDGE: ServiceToken<Bridge> = ServiceToken::new("helios.config");

/// Where the workspace's own values of egui-pop settings live (the
/// `workspace` value scope), beside its other files.
pub const WORKSPACE_FILE: &str = "settings.json";

#[derive(Default)]
struct State {
    settings: Option<Settings>,
    vault: Option<VaultWriter>,
    config_dir: Option<PathBuf>,
    /// The open workspace's settings file and its values as last saved.
    workspace: Option<(PathBuf, BTreeMap<String, Json>)>,
    problems: Vec<String>,
}

/// The service: see [`install`] and [`open_workspace`].
#[derive(Default)]
pub struct Bridge {
    state: Mutex<State>,
    /// A bridged setting's store value changed since the last pass.
    dirty: Arc<AtomicBool>,
}

impl Bridge {
    /// What writing the layers back said, last first.
    pub fn problems(&self) -> Vec<String> {
        lock(&self.state).problems.clone()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

const PLUGIN: PluginId = PluginId::from_static("helios.config");

/// A declared setting's egui-pop descriptor.
pub fn descriptor(s: &helios_config::Setting) -> Descriptor {
    let d = match s.kind {
        Kind::Number {
            min,
            max,
            default,
            unit,
            integer: true,
        } => Descriptor::int(s.label, min as i64, max as i64, default as i64).unit(unit),
        Kind::Number {
            min,
            max,
            default,
            unit,
            integer: false,
        } => Descriptor::float(s.label, min, max, default).unit(unit),
        Kind::Toggle { default, .. } => Descriptor::bool(s.label, default),
        Kind::Choice { options, default } => Descriptor::choice(s.label, options, default),
        Kind::Text { default } => Descriptor::text(s.label, default).capacity(4096),
    };
    d.description(s.about)
}

/// A value as JSON, in the descriptor's type.
fn to_json(kind: &Kind, v: &Value) -> Json {
    match (kind, v) {
        (Kind::Number { integer: true, .. }, Value::Number(n)) => json!(*n as i64),
        (_, Value::Number(n)) => json!(n),
        (_, Value::Toggle(b)) => json!(b),
        (_, Value::Choice(c)) => json!(c),
    }
}

fn from_json(kind: &Kind, j: &Json) -> Option<Value> {
    match kind {
        Kind::Number { .. } => j.as_f64().map(Value::Number),
        Kind::Toggle { .. } => j.as_bool().map(Value::Toggle),
        Kind::Choice { .. } | Kind::Text { .. } => j.as_str().map(|s| Value::Choice(s.into())),
    }
}

/// The value scopes a concern's settings may be set in.
pub fn scopes(concern: Concern) -> &'static [ValueScope] {
    match concern {
        Concern::Machine | Concern::User => &[ValueScope::App],
        Concern::Project => &[ValueScope::App, ValueScope::Library],
    }
}

/// Where each declared setting goes: scope, section, its title.
pub const PLACES: &[(&str, &str, &[&str])] = &[
    (
        "interface/workbench",
        "Workbench",
        &["interface.panels.queue_when_empty", "interface.graph.zoom"],
    ),
    (
        "interface/experimental",
        "Experimental",
        &["interface.experimental.workshops"],
    ),
    (
        "library/image-map",
        "Image map",
        &[
            "system.map.record",
            "system.map.defaults",
            "system.map.exclude",
            "system.map.limit",
            "system.map.keepMissing",
        ],
    ),
    (
        "library/decisions",
        "Decisions",
        &["project.decisions.mode"],
    ),
    ("library/history", "History", &["system.workflow.keep_days"]),
    (
        "storage/caches",
        "Caches",
        &[
            "system.cache.budget",
            "system.cache.share",
            "system.cache.thumbnails",
            "system.cache.textures",
        ],
    ),
    (
        "hardware/workers",
        "Workers",
        &[
            "system.scheduler.workers",
            "system.ai.calls",
            "system.events.journal",
        ],
    ),
];

/// The manifest: Helios's scopes, in the note's order, and its settings
/// placed in them (the display's and the backup destinations' egui-pop
/// settings too).
pub fn manifest(schema: &Schema) -> Manifest {
    let mut m = Manifest::new(FORMAT)
        .scope_at("library", "Library", "folder", 30)
        .scope_at("storage", "Storage", "disc", 40)
        .scope_at("backup", "Backup", "lock", 50)
        .scope_at("hardware", "Hardware", "command", 70);
    for (path, title, keys) in PLACES {
        m = m.section(path, title, |s| {
            for key in *keys {
                let Some(setting) = schema.get(key) else {
                    continue;
                };
                // Every part reads them once, when the desktop starts.
                s.field(
                    Field::setting(key)
                        .scopes(scopes(setting.concern))
                        .restart(),
                );
            }
        });
    }
    m.section("hardware/display", "Display", |s| {
        s.field(Field::setting(crate::display::SETTING_WIDE_GAMUT.as_str()));
    })
    .section("backup/destinations", "Destinations", |s| {
        s.field(Field::setting(
            crate::platform::SETTING_DESTINATION_DAYS.as_str(),
        ));
    })
}

/// The plugin: declares the settings and the manifest, follows their
/// changes. Values come once the desktop [`install`]s them; until then
/// the settings show their defaults and nothing is written.
#[derive(Debug, Default)]
pub struct BridgePlugin;

impl Plugin for BridgePlugin {
    fn id(&self) -> PluginId {
        PLUGIN
    }

    fn title(&self) -> String {
        "Helios settings".into()
    }

    fn requires(&self) -> Vec<PluginId> {
        vec![PluginId::from_static("egui_pop.admin")]
    }

    fn connect(&mut self, ctx: &mut Ctx<'_>) -> Result<(), PluginError> {
        let schema = Schema::core();
        for s in schema.iter() {
            ctx.setting(setting_id(s.key)?, descriptor(s))?;
        }
        let admin = ctx
            .app()
            .service(&ADMIN)
            .ok_or_else(|| PluginError::Failed("no Settings app".to_string()))?;
        admin.manifest(manifest(&schema));
        let bridge = Bridge::default();
        let dirty = bridge.dirty.clone();
        ctx.service(&BRIDGE, bridge)?;
        let keys: BTreeSet<String> = schema
            .iter()
            .map(|s| format!("settings/{}", s.key))
            .collect();
        ctx.on_change("settings/", move |c| {
            if keys.contains(&c.path) {
                dirty.store(true, Ordering::Relaxed);
            }
        });
        ctx.on(On::Pass, |app, _| {
            let (Some(bridge), Some(admin)) = (app.service(&BRIDGE), app.service(&ADMIN)) else {
                return;
            };
            if bridge.dirty.swap(false, Ordering::Relaxed) || admin.is_open() {
                sync(app, &bridge, &admin);
            }
        });
        Ok(())
    }
}

fn setting_id(key: &str) -> Result<SettingId, PluginError> {
    SettingId::new(key).map_err(|e| PluginError::Failed(format!("{key}: {e:?}")))
}

/// Writes a store value with no undo step (a value read from a layer,
/// not an edit).
fn write_store(app: &AppHandle, key: &str, v: &Json) {
    let path = format!("settings/{key}");
    let store = app.store();
    let mut s = egui_pop::core::lock(&store);
    let Some(ty) = s.get(&path).map(|e| e.descriptor.ty) else {
        return;
    };
    if let Ok(v) = PopValue::from_json(ty, v) {
        let _ = s.set(
            Set::new(&path, Change::To(v), Origin::User)
                .force()
                .untracked()
                .by(&OwnerId::FRAMEWORK),
        );
    }
}

/// Hands the resolved settings to the Settings app: their `app` values,
/// the library's layer (offered when a library is open) and `--set`.
/// `vault` writes the vault's layers back; `config_dir` holds the
/// machine's file.
pub fn install(app: &AppHandle, settings: Settings, vault: Option<VaultWriter>, config_dir: &Path) {
    let (Some(bridge), Some(admin)) = (app.service(&BRIDGE), app.service(&ADMIN)) else {
        return;
    };
    let schema = Schema::core();
    for s in schema.iter() {
        if let Some(v) = settings.app_value(s.key) {
            write_store(app, s.key, &to_json(&s.kind, &v));
        }
    }
    let project = settings.project().is_some();
    admin.offer(ValueScope::Library, project);
    let library: BTreeMap<String, Json> = schema
        .iter()
        .filter(|s| s.concern == Concern::Project)
        .filter_map(|s| {
            let v = settings.config().value_in(Layer::Project, s.key)?;
            Some((s.key.to_string(), to_json(&s.kind, &v)))
        })
        .collect();
    admin.load_layer(app, ValueScope::Library, library);
    for s in schema.iter() {
        if let Some(v) = settings.config().value_in(Layer::Run, s.key)
            && let Ok(id) = SettingId::new(s.key)
            && let Err(e) = admin.set_run(app, &id, to_json(&s.kind, &v))
        {
            log::warn!("--set {}: {e}", s.key);
        }
    }
    let mut st = lock(&bridge.state);
    st.settings = Some(settings);
    st.vault = vault;
    st.config_dir = Some(config_dir.to_path_buf());
    drop(st);
    bridge.dirty.store(false, Ordering::Relaxed);
}

/// Opens a workspace's own values of the settings (the `workspace` value
/// scope), kept in `file`; `None` closes it.
pub fn open_workspace(app: &AppHandle, file: Option<PathBuf>) {
    let (Some(bridge), Some(admin)) = (app.service(&BRIDGE), app.service(&ADMIN)) else {
        return;
    };
    let values: BTreeMap<String, Json> = file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    admin.offer(ValueScope::Workspace, file.is_some());
    admin.load_layer(app, ValueScope::Workspace, values.clone());
    lock(&bridge.state).workspace = file.map(|f| (f, values));
}

/// Takes the Settings app's values back to their layers, and writes the
/// layers that changed.
fn sync(app: &AppHandle, bridge: &Bridge, admin: &Admin) {
    let mut st = lock(&bridge.state);
    let layers = admin.layers();
    let store = app.store();
    let mut problems = Vec::new();
    if let Some(settings) = st.settings.as_mut() {
        let schema = Schema::core();
        let values: Vec<(&helios_config::Setting, Json)> = {
            let s = egui_pop::core::lock(&store);
            schema
                .iter()
                .filter_map(|x| {
                    let e = s.get(&format!("settings/{}", x.key))?;
                    Some((x, e.value.to_json()))
                })
                .collect()
        };
        let library = settings.project().is_some();
        for (s, now) in values {
            let path = format!("settings/{}", s.key);
            let home = Settings::home(s.concern);
            let app_v = layers
                .value_in(ValueScope::App, &path, &now)
                .and_then(|j| from_json(&s.kind, &j));
            // The app value equal to what is under the home layer leaves
            // it (`Config::set`), so defaults keep evolving.
            if let Err(e) = settings.set(home, s.key, app_v) {
                problems.push(e);
            }
            if s.concern == Concern::Project && library {
                let lib_v = layers
                    .value_in(ValueScope::Library, &path, &now)
                    .and_then(|j| from_json(&s.kind, &j));
                if let Err(e) = settings.set(Layer::Project, s.key, lib_v) {
                    problems.push(e);
                }
            }
        }
        let project = settings.project().map(String::from);
        for (layer, text) in settings.take_changed() {
            let written = match layer {
                Layer::Global => match &st.config_dir {
                    Some(dir) => std::fs::create_dir_all(dir)
                        .and_then(|()| std::fs::write(dir.join(GLOBAL_FILE), &text))
                        .map_err(|e| format!("{}: {e}", dir.join(GLOBAL_FILE).display())),
                    None => Ok(()),
                },
                Layer::User | Layer::Project => {
                    let name = if layer == Layer::User {
                        Some(USER_LAYER.to_string())
                    } else {
                        project.clone()
                    };
                    match (&st.vault, name) {
                        (Some(w), Some(name)) => w(&name, &text),
                        _ => Ok(()),
                    }
                }
                _ => Ok(()),
            };
            if let Err(e) = written {
                problems.push(format!("the {} settings: {e}", layer.name()));
            }
        }
    }
    let workspace = admin.export_layer(ValueScope::Workspace);
    if let Some((file, saved)) = st.workspace.as_mut()
        && *saved != workspace
    {
        let text = serde_json::to_string_pretty(&workspace).unwrap_or_default();
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&*file, text));
        match written {
            Ok(()) => *saved = workspace,
            Err(e) => problems.push(format!("{}: {e}", file.display())),
        }
    }
    for p in &problems {
        log::warn!("settings: {p}");
    }
    if !problems.is_empty() {
        st.problems = problems;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(user: &str) -> Sources {
        Sources {
            user: Some(user.into()),
            ..Sources::default()
        }
    }

    #[test]
    fn a_machine_value_left_in_the_user_layer_applies() {
        // `helios-node --user` kept this in the vault's `user` layer; the
        // user layer refuses machine settings, so it never applied.
        let s = Settings::resolve(&sources(
            "[system.cache]\nthumbnails = 4096\ntextures = 64\n",
        ));
        assert_eq!(s.mib("system.cache.thumbnails"), 4096 << 20);
        assert_eq!(s.mib("system.cache.textures"), 64 << 20);
        assert_eq!(s.problems().len(), 2, "{:?}", s.problems());
    }

    #[test]
    fn every_layer_counts_in_its_concern_order() {
        let s = Settings::resolve(&Sources {
            global: Some(
                "[interface.graph]\nzoom = \"overview\"\n[system.map]\nrecord = \"import\"\n"
                    .into(),
            ),
            project: Some((
                "project//photos".into(),
                Some("[project.decisions]\nmode = \"human\"\n".into()),
            )),
            user: Some("[interface.graph]\nzoom = \"detail\"\n".into()),
            run: vec![("system.map.record".into(), "everywhere".into())],
        });
        assert!(s.problems().is_empty(), "{:?}", s.problems());
        let c = s.config();
        assert_eq!(c.get("interface.graph.zoom").unwrap().1, Layer::User);
        assert_eq!(c.choice("project.decisions.mode"), "human");
        assert_eq!(c.get("system.map.record").unwrap().1, Layer::Run);
        assert_eq!(s.project(), Some("project//photos"));
    }

    #[test]
    fn a_change_is_taken_once_and_a_moved_value_saves_both_layers() {
        let mut s = Settings::resolve(&sources("[system.cache]\nthumbnails = 4096\n"));
        // The moved value now lives in the machine's layer and leaves the
        // user's.
        let changed: Vec<Layer> = s.take_changed().into_iter().map(|(l, _)| l).collect();
        assert_eq!(changed, [Layer::Global, Layer::User]);
        assert!(s.take_changed().is_empty());
        s.set(
            Layer::User,
            "interface.graph.zoom",
            Some(Value::Choice("overview".into())),
        )
        .unwrap();
        let changed = s.take_changed();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, Layer::User);
        assert!(changed[0].1.contains("overview"));
        // A machine setting is refused in the user layer.
        assert!(
            s.set(Layer::User, "system.ai.calls", Some(Value::Number(3.0)))
                .is_err()
        );
    }

    #[test]
    fn every_setting_is_placed_once_with_the_scopes_of_its_concern() {
        let schema = Schema::core();
        let m = manifest(&schema);
        let bound: Vec<&str> = m.bound_settings().collect();
        for s in schema.iter() {
            assert_eq!(
                bound.iter().filter(|b| **b == s.key).count(),
                1,
                "{} placed once",
                s.key
            );
        }
        let decisions = m
            .sections
            .iter()
            .flat_map(|s| &s.groups)
            .flat_map(|g| &g.items)
            .find_map(|i| match i {
                egui_pop::api::admin::model::Item::Field(f)
                    if f.setting_id() == Some("project.decisions.mode") =>
                {
                    Some(f.scopes.clone())
                }
                _ => None,
            });
        assert_eq!(decisions, Some(vec![ValueScope::App, ValueScope::Library]));
        // Every descriptor reads its own default back.
        for s in schema.iter() {
            let d = descriptor(s);
            let j = to_json(&s.kind, &s.kind.default_value());
            assert_eq!(d.default.to_json(), j, "{}", s.key);
            assert_eq!(from_json(&s.kind, &j), Some(s.kind.default_value()));
        }
    }

    #[test]
    fn a_library_names_its_project_layer_by_its_folder() {
        let dir = std::env::temp_dir();
        let name = project_layer(&dir.join("library.helios")).unwrap();
        assert!(name.starts_with("project/"));
        assert!(!name.ends_with("library.helios"));
    }
}
