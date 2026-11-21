//! Helios's own settings (`helios-config`) for the desktop: resolved once
//! from their layers, read by every part through [`Settings`], and shown
//! in the Settings app ([`Bridge`]).
//!
//! **Layers, as `helios-config` says** ([`helios_config::Concern::layers`]):
//!
//! | layer | where the desktop keeps it |
//! |---|---|
//! | `global` | this machine: `<config>/global.toml` ([`GLOBAL_FILE`]) |
//! | `project` | the library: the node's `project/<library folder>` layer |
//! | `user` | the person, on every machine: the node's `user` layer |
//! | `run` | the command line's `--set KEY=VALUE` |
//!
//! The node keeps the `user` and `project` layers in its vault and every
//! client reads and writes them through it
//! (`helios_node_core::settings`); the machine's file and the
//! interface's own settings (egui-pop's) stay on the client.
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
//! `--set` its `command line` origin. The Settings app keeps them in the
//! store Helios gives it ([`store`]): edits are saved to their layer away
//! from the UI thread; they take effect at the next start.
//!
//! **Followed live.** A layer the node keeps that another of its clients
//! changed (`helios.settings.layerChanged` on its bus) is read again
//! ([`follow`]): the Settings app shows its new
//! values, and nothing is written back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use egui_pop::api::admin::model::{
    Action, Clauses as _, Confirmation, FORMAT, Field, FieldKind, Manifest, Records, RowField,
    SectionBuilder, Status, ValueScope, Wizard,
};
use egui_pop::api::admin::store::SettingsStore as _;
use egui_pop::api::admin::{ADMIN, Admin, ExtraScope};
use egui_pop::api::services::ServiceToken;
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::core::strings::StrKey;
use egui_pop::model::ids::{PluginId, SettingId};
use egui_pop::model::values::Descriptor;
use helios_config::{Concern, Config, Kind, Layer, Schema, Value};
use helios_core::Message;
use serde_json::{Value as Json, json};

pub mod focus;
pub mod plugin_pages;
pub mod plugins;
pub mod store;

pub use store::{INTERFACE_FILE, NodeLayers, NodeSettingsStore};

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

    /// The node layer named `name`, when it is one these settings read:
    /// the person's, or the open library's project layer.
    pub fn node_layer(&self, name: &str) -> Option<Layer> {
        if name == USER_LAYER {
            Some(Layer::User)
        } else if self.project.as_deref() == Some(name) {
            Some(Layer::Project)
        } else {
            None
        }
    }

    /// Reads a layer the node keeps again, from its text there now
    /// (`None`: it keeps none): its values replace the ones held, and it
    /// counts as saved. Whether any of its values changed. What does not
    /// read is left out and logged, as at the start.
    fn reload(&mut self, layer: Layer, name: &str, text: Option<&str>) -> bool {
        let before = self.config.layer_toml(layer);
        let keys: Vec<&'static str> = self.config.schema().iter().map(|s| s.key).collect();
        for key in keys {
            self.config.unset(layer, key);
        }
        if let Some(text) = text {
            let l = self.config.load_str_lenient(layer, text);
            for key in l.moved {
                let layer = layer.name();
                log::warn!(
                    target: "helios",
                    "the settings: {name}: `{key}` is not set in the {layer} layer; read as this machine's"
                );
            }
            for p in &l.problems {
                log::warn!(target: "helios", "the settings: {name}: {p}");
            }
        }
        let after = self.config.layer_toml(layer);
        self.saved.insert(layer, after.clone());
        before != after
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

/// The Settings app's side: the settings as egui-pop settings, their
/// manifest, and the values written back to their layers.
pub const BRIDGE: ServiceToken<Bridge> = ServiceToken::new("helios.config");

/// Where the workspace's own values of egui-pop settings live (the
/// `workspace` value scope), beside its other files.
pub const WORKSPACE_FILE: &str = "settings.json";

#[derive(Default)]
struct State {
    /// Helios's settings and the configuration folder, once installed.
    installed: Option<(NodeSettingsStore, PathBuf)>,
    /// The plugins' settings, once installed.
    plugins: Option<plugins::PluginSettingsStore>,
    /// The open workspace's settings file.
    workspace: Option<PathBuf>,
    /// The open workspace's name and the library's, which the Settings
    /// header names its focus by.
    workspace_name: Option<String>,
    library: Option<String>,
    /// The Settings edit the import window's focus (the library alone),
    /// not the main window's ([`focus`]).
    import_edited: bool,
    /// Where the plugins' secrets are kept: the system keyring unless
    /// [`use_secrets`] says otherwise.
    secrets: Option<Arc<dyn helios_connect::account::Secrets>>,
}

/// The service: see [`install`] and [`open_workspace`].
#[derive(Default)]
pub struct Bridge {
    state: Mutex<State>,
    /// The plugins whose settings are declared.
    plugins: Vec<plugins::PluginSettings>,
}

/// Keeps the plugins' secrets in `secrets` instead of the system
/// keyring (tests); before [`install`].
pub fn use_secrets(app: &AppHandle, secrets: Arc<dyn helios_connect::account::Secrets>) {
    if let Some(bridge) = app.service(&BRIDGE) {
        lock(&bridge.state).secrets = Some(secrets);
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
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

/// The value scope of the open library: a value set for it wins over
/// the app's while that library is open.
pub const LIBRARY: ExtraScope = ExtraScope::new(
    "library",
    StrKey::new("gui-settings-for-library", "This library"),
    StrKey::new(
        "gui-settings-for-library-about",
        "while this library is open",
    ),
);

/// The value scope of the open workspace: a value set for it wins over
/// the library's and the app's while that workspace is open.
pub const WORKSPACE: ExtraScope = ExtraScope::new(
    "workspace",
    StrKey::new("gui-settings-for-workspace", "This workspace"),
    StrKey::new(
        "gui-settings-for-workspace-about",
        "while this workspace is open",
    ),
);

/// The value scopes Helios declares above the app's, lowest first (a
/// later one wins). A setting is app-wide unless it opts into one.
pub const SCOPES: [ExtraScope; 2] = [LIBRARY, WORKSPACE];

/// The value scopes above the app's a concern's settings opt into: a
/// library's defaults may differ per library; everything else is
/// app-wide.
pub fn scopes(concern: Concern) -> &'static [ValueScope] {
    match concern {
        Concern::Machine | Concern::User => &[],
        Concern::Project => &[LIBRARY.scope],
    }
}

/// The settings offered in the quick settings (egui-pop's `admin.quick`),
/// in this order: the few a person changes often, without opening
/// Settings. Those the person changed lately follow them.
pub const QUICK: &[&str] = &[
    "appearance.theme_preference",
    "appearance.interface_zoom",
    "appearance.text_size",
    "appearance.language",
    "appearance.accent_color",
    "display.wide-gamut-preview",
    "interface.graph.zoom",
    "interface.panels.queue_when_empty",
    "project.decisions.mode",
    "system.ai.calls",
    "notifications.jobs",
    "interface.experimental.workshops",
    helios_library::auto_import::PAUSED,
];

/// Where each declared setting goes: scope and section.
pub const PLACES: &[(&str, &[&str])] = &[
    (
        "interface/workbench",
        &["interface.panels.queue_when_empty", "interface.graph.zoom"],
    ),
    (
        "interface/experimental",
        &["interface.experimental.workshops"],
    ),
    (
        "library/image-map",
        &[
            "system.map.record",
            "system.map.defaults",
            "system.map.exclude",
            "system.map.limit",
            "system.map.keepMissing",
        ],
    ),
    ("library/imports", &[crate::shell::SHOW_IMPORT_SETTING]),
    ("library/decisions", &["project.decisions.mode"]),
    ("library/history", &["system.workflow.keep_days"]),
    (
        "storage/caches",
        &[
            "system.cache.budget",
            "system.cache.share",
            "system.cache.states",
            "system.cache.thumbnails",
            "system.cache.textures",
        ],
    ),
    (
        "hardware/workers",
        &[
            "system.scheduler.workers",
            "system.ai.calls",
            "system.events.journal",
        ],
    ),
];

/// The automatic imports' safeguards, on their own section of the
/// Library page ([`helios_library::auto_import`]).
pub const AUTOMATIC_IMPORTS: &[&str] = &[
    helios_library::auto_import::PAUSED,
    helios_library::auto_import::ASK_FILES,
    helios_library::auto_import::ASK_GB,
];

/// A scope's or a section's title, by its path, in the catalogues
/// (`cargo xtask i18n extract` reads them here).
pub fn title(path: &str) -> Message {
    match path {
        "library" => Message::new("gui-settings-library", "Library"),
        "storage" => Message::new("gui-settings-storage", "Storage"),
        "backup" => Message::new("gui-settings-backup", "Backup"),
        "hardware" => Message::new("gui-settings-hardware", "Hardware"),
        "health" => Message::new("gui-settings-health", "Health"),
        "health/check" => Message::new("gui-settings-health-check", "Health check"),
        "accounts" => Message::new("gui-settings-accounts", "Accounts"),
        "accounts/list" => Message::new("gui-settings-accounts-list", "Import accounts"),
        "accounts/clients" => {
            Message::new("gui-settings-accounts-clients", "Use my own OAuth client")
        }
        "plugins" => Message::new("gui-settings-plugins", "Plugins"),
        "interface/workbench" => Message::new("gui-settings-workbench", "Workbench"),
        "interface/experimental" => Message::new("gui-settings-experimental", "Experimental"),
        "library/image-map" => Message::new("gui-settings-image-map", "Image map"),
        "library/imports" => Message::new("gui-settings-imports", "Imports"),
        "library/decisions" => Message::new("gui-settings-decisions", "Decisions"),
        "library/history" => Message::new("gui-settings-history", "History"),
        "library/automatic-imports" => {
            Message::new("gui-settings-automatic-imports", "Automatic imports")
        }
        "storage/volumes" => Message::new("gui-settings-volumes", "Volumes"),
        "storage/caches" => Message::new("gui-settings-caches", "Caches"),
        "storage/catalogue" => Message::new("gui-settings-catalogue", "Catalogue"),
        "hardware/workers" => Message::new("gui-settings-workers", "Workers"),
        "hardware/display" => Message::new("gui-settings-display", "Display"),
        "backup/strategy" => Message::new("gui-settings-strategy", "Strategy"),
        "backup/destinations" => Message::new("gui-settings-destinations", "Destinations"),
        "danger" => Message::new("gui-settings-danger", "Danger zone"),
        "danger/reset" => Message::new("gui-settings-reset", "Reset Helios"),
        other => Message::from(other.to_string()),
    }
}

/// The manifest: Helios's scopes, in the note's order, and its settings
/// placed in them (the display's, the catalogue's and the backup
/// destinations' egui-pop settings too), with the Storage and Backup
/// pages read from the storage commands, and a page per plugin installed
/// (`plugins`: its permissions and its settings, from its contract); its
/// texts through `t` (the interface's language).
pub fn manifest(
    schema: &Schema,
    plugins: &[plugins::PluginSettings],
    t: &dyn Fn(Message) -> String,
) -> Manifest {
    let tt = |path: &str| t(title(path));
    let mut m = Manifest::new(FORMAT)
        .scope_at("library", &tt("library"), "folder", 30)
        .scope_at("storage", &tt("storage"), "disc", 40)
        .scope_at("backup", &tt("backup"), "lock", 50)
        .scope_at("accounts", &tt("accounts"), "unlock", 60)
        .scope_at("plugins", &tt("plugins"), "phosphor:puzzle-piece", 65)
        .scope_at("hardware", &tt("hardware"), "command", 70)
        .scope_at("health", &tt("health"), "success", 90)
        // Last: what cannot be had back easily.
        .scope_at("danger", &tt("danger"), "warning", 99)
        .quick(QUICK);
    m = m.section("storage/volumes", &tt("storage/volumes"), |s| {
        helios_storage_ui::settings::volumes_section(s, t, crate::shell::FORGET_VOLUME);
    });
    for (path, keys) in PLACES {
        m = m.section(path, &tt(path), |s| {
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
            if *path == "storage/caches" {
                caches_section(s, t);
            }
        });
    }
    // Read as they change, by whichever runs the automatic imports: no
    // restart.
    m = m.section(
        "library/automatic-imports",
        &tt("library/automatic-imports"),
        |s| {
            for key in AUTOMATIC_IMPORTS {
                if let Some(setting) = schema.get(key) {
                    s.field(Field::setting(key).scopes(scopes(setting.concern)));
                }
            }
        },
    );
    let m = m.section("storage/catalogue", &tt("storage/catalogue"), |s| {
        s.description(&t(Message::new(
            "gui-settings-catalogue-about",
            "Verified snapshots of the catalogue (ratings, tags, edits): kept next to it and copied to the backup disks plugged in.",
        )));
        s.field(Field::setting(crate::catalogue::SETTING_EVERY.as_str()));
        s.field(Field::setting(crate::catalogue::SETTING_KEPT.as_str()));
        snapshots_section(s, t);
    })
    .section("hardware/display", &tt("hardware/display"), |s| {
        s.field(Field::setting(crate::display::SETTING_WIDE_GAMUT.as_str()));
        screens_section(s, t);
    })
    .section("backup/strategy", &tt("backup/strategy"), |s| {
        helios_storage_ui::settings::strategy_section(s, t);
    })
    .section("backup/destinations", &tt("backup/destinations"), |s| {
        s.field(Field::setting(
            crate::platform::SETTING_DESTINATION_DAYS.as_str(),
        ));
    })
    .section("health/check", &tt("health/check"), |s| {
        health_section(s, plugins, t);
    })
    .section("accounts/list", &tt("accounts/list"), |s| {
        accounts_section(s, t);
    })
    .section("accounts/clients", &tt("accounts/clients"), |s| {
        clients_section(s, t);
    })
    .section(crate::reset::SECTION, &tt(crate::reset::SECTION), |s| {
        crate::reset::section(s, t);
    })
    .wizard(helios_storage_ui::settings::backup_wizard(t))
    .wizard(crate::reset::wizard(t))
    .wizard(crate::forget::wizard(t))
    .wizard(init_wizard(&helios_init::Registry::builtin(), t));
    plugins::sections(m, plugins, t)
}

/// The first-run setup (`crate::shell` `init_wizard`): a step per init
/// step, in order; its fields are the settings the step fills, as their
/// declarations say (`helios_config::KEPT_ELSEWHERE`), required on a
/// required step; an optional step is skipped ("Later"). The library
/// folder is a path field with "Choose…" (the folder chooser) and a
/// check that it can be written, with room left. No summary: Finish on
/// the last step writes the settings (`init.apply`).
pub fn init_wizard(registry: &helios_init::Registry, t: &dyn Fn(Message) -> String) -> Wizard {
    use crate::shell::init_wizard::{APPLY, CHECK_FOLDER, CHOOSE, WIZARD};
    let mut w = Wizard::new(
        WIZARD,
        &t(Message::new("gui-init-wizard", "Set up Helios")),
        APPLY,
    )
    .description(&t(Message::new(
        "gui-init-wizard-about",
        "A few choices before the first photos: each can be changed later.",
    )))
    .no_summary()
    .verb(&t(Message::new("gui-init-wizard-finish", "Finish")));
    let steps = registry.ordered().unwrap_or_else(|e| {
        log::warn!("init: {e}");
        Vec::new()
    });
    for step in steps {
        w = w.step(&step.id, &t(step.title.clone()), |s| {
            s.description(&t(step.why.clone()));
            for key in &step.fills {
                let kind = match helios_config::kept_elsewhere(key)
                    .map(|d| d.kind)
                    .or_else(|| Schema::core().get(key).map(|d| d.kind))
                {
                    Some(Kind::Toggle { .. }) => FieldKind::Toggle,
                    Some(Kind::Number { .. }) => FieldKind::Number,
                    Some(Kind::Choice { .. }) => FieldKind::Choice,
                    _ if key == helios_init::LIBRARY_FOLDER_SETTING => FieldKind::Path,
                    _ => FieldKind::Text,
                };
                let mut field = Field::draft(WIZARD, key, &t(step.title.clone()), kind);
                if step.required {
                    field = field.required();
                }
                if key == helios_init::LIBRARY_FOLDER_SETTING {
                    field = field.validate(CHECK_FOLDER).help(&t(Message::new(
                        "gui-init-library-folder-help",
                        "A folder on a disk with room for your photos; it is made if missing.",
                    )));
                }
                s.field(field);
                if key == helios_init::LIBRARY_FOLDER_SETTING {
                    s.action(Action::command(CHOOSE).title(&t(Message::new(
                        "gui-init-library-folder-choose",
                        "Choose…",
                    ))));
                }
            }
        });
        // Hidden out of a partial run (`init_wizard::skip_key`).
        w = w.when(&format!(
            "!form.{}",
            crate::shell::init_wizard::skip_key(&step.id)
        ));
        if !step.required {
            w = w.optional();
        }
    }
    reimport_step(w, t)
}

/// The setup's last step, shown after an "everything" reset only (the
/// draft's `init.reimport.offered`, `crate::shell::init_wizard`): the
/// photo folders the reset kept, and a toggle, on, to add their photos
/// back where they are. Skipped ("Later"), nothing is added.
fn reimport_step(w: Wizard, t: &dyn Fn(Message) -> String) -> Wizard {
    use crate::shell::init_wizard::{
        REIMPORT_FOLDERS, REIMPORT_KEY, REIMPORT_OFFERED, REIMPORT_STEP, WIZARD,
    };
    w.step(
        REIMPORT_STEP,
        &t(Message::new("gui-init-reimport", "Your photos")),
        |s| {
            s.description(&t(Message::new(
                "gui-init-reimport-about",
                "Helios was reset. Its photos are still in these folders: add them back to the library where they are. Nothing is copied or moved.",
            )));
            s.list(
                Records::command("init-reimport-folders", "", REIMPORT_FOLDERS, "/folders", "id")
                    .titles("path", None),
            );
            s.field(
                Field::draft(
                    WIZARD,
                    REIMPORT_KEY,
                    &t(Message::new(
                        "gui-init-reimport-toggle",
                        "Add these photos back, where they are",
                    )),
                    FieldKind::Toggle,
                )
                .default_value(Json::Bool(false)),
            );
        },
    )
    .when(&format!("form.{REIMPORT_OFFERED}"))
    .optional()
}

/// The `validate` hook of the setup's library folder field: what is wrong
/// with the folder typed, read without writing anything.
fn check_library_folder(
    t: impl Fn(Message) -> String + Send + Sync + 'static,
) -> egui_pop::api::admin::model::hooks::Hook {
    use egui_pop::api::admin::model::Tone;
    use egui_pop::api::admin::model::hooks::{Hook, Problem};
    Hook::Validate(Box::new(move |values| {
        let key = helios_init::LIBRARY_FOLDER_SETTING;
        let Some(path) = values
            .get(key)
            .and_then(Json::as_str)
            .filter(|p| !p.trim().is_empty())
        else {
            return Vec::new();
        };
        helios_library::import::library_folder_problem(Path::new(path.trim()))
            .map(|p| Problem {
                field: key.to_owned(),
                tone: Tone::Danger,
                message: t(p.message()),
            })
            .into_iter()
            .collect()
    }))
}

/// The owners of health checks the Health page has a group for, in its
/// order: Helios's own modules, then each plugin installed. Another
/// owner's findings go in the page's last group, "Other checks".
pub fn health_owners(plugins: &[plugins::PluginSettings]) -> Vec<String> {
    crate::shell::HEALTH_OWNERS
        .iter()
        .map(|&owner| owner.to_owned())
        .chain(plugins.iter().map(|p| p.id.clone()))
        .collect()
}

/// The words for the owner `owner` of health checks: a module's name, or
/// a plugin's title.
fn health_owner_title(owner: &str, plugins: &[plugins::PluginSettings]) -> Message {
    match owner {
        "storage" => Message::new("gui-health-owner-storage", "Storage"),
        "library" => Message::new("gui-health-owner-library", "Library"),
        "backup" => Message::new("gui-health-owner-backup", "Backups"),
        "gpu" => Message::new("gui-health-owner-gpu", "GPU"),
        "plugins" => Message::new("gui-health-owner-plugins", "Plugins"),
        "catalogue" => Message::new("gui-health-owner-catalogue", "Catalogue"),
        _ => Message::plain(
            plugins
                .iter()
                .find(|p| p.id == owner)
                .map_or(owner, |p| p.title.as_str()),
        ),
    }
}

/// The Health scope: how it stands, "Check now", and the findings of the
/// last run (`health.findings`) grouped by owner (the module or the
/// plugin whose check found them, [`health_owners`]; a group shows once
/// it has findings), the worst first, toned by their level, each with its
/// fix ("Fix…", asked first when it changes something not easily taken
/// back) and "Ignore"; the ignored ones last, with "Show again".
fn health_section(
    s: &mut SectionBuilder<'_>,
    plugins: &[plugins::PluginSettings],
    t: &dyn Fn(Message) -> String,
) {
    use crate::shell::{
        HEALTH_CHECK_NOW, HEALTH_FINDINGS, HEALTH_FIX, HEALTH_IGNORE, HEALTH_OTHERS,
        health_owner_key,
    };
    s.description(&t(Message::new(
        "gui-health-about",
        "Helios checks the backups, the disks, the GPU, the plugins and the catalogue snapshots in the background: a while after start, daily, and when a disk comes or goes.",
    )));
    s.status(Status::command(
        "summary",
        &t(Message::new("gui-health-status", "Status")),
        HEALTH_FINDINGS,
        "/summary",
    ));
    s.action(
        Action::command(HEALTH_CHECK_NOW)
            .title(&t(Message::new("gui-health-check-now", "Check now")))
            .primary(),
    );
    let id = json!({ "id": "${row.id}" });
    let fix = |destructive: bool| {
        let a = Action::command(HEALTH_FIX)
            .title(&t(Message::new("gui-health-fix", "Fix…")))
            .args(id.clone());
        if destructive {
            a.when("row.fix && row.destructive").danger().confirmation(
                Confirmation::new(
                    "${row.fixLabel}",
                    "${row.title}",
                    &t(Message::new("gui-health-fix-confirm", "Fix it")),
                )
                .danger(),
            )
        } else {
            a.when("row.fix && !row.destructive")
        }
    };
    let list = |id: &str, pointer: &str| {
        Records::command(id, "", HEALTH_FINDINGS, pointer, "id")
            .titles("title", Some("text"))
            .column("fixLabel", "")
            .column("ignoredText", "")
            .tone("tone")
            .row_action(fix(false))
            .row_action(fix(true))
            .row_action(
                Action::command(HEALTH_IGNORE)
                    .title(&t(Message::new("gui-health-ignore", "Ignore")))
                    .args(json!({ "id": "${row.id}", "on": true }))
                    .when("row.attention && !row.ignored"),
            )
            .row_action(
                Action::command(HEALTH_IGNORE)
                    .title(&t(Message::new("gui-health-undo-ignore", "Show again")))
                    .args(json!({ "id": "${row.id}", "on": false }))
                    .when("row.ignored"),
            )
    };
    for (i, owner) in health_owners(plugins).iter().enumerate() {
        s.group(
            &format!("owner-{i}"),
            &t(health_owner_title(owner, plugins)),
        );
        s.group_when(&health_owner_key(owner));
        s.list(list(&format!("findings-{i}"), &format!("/owners/{owner}")));
    }
    s.group(
        "owner-others",
        &t(Message::new("gui-health-owner-other", "Other checks")),
    );
    s.group_when(HEALTH_OTHERS);
    s.list(list("findings-others", "/others"));
}

/// The Caches page's usage: what each cache holds (`cache.stats`), per
/// kind, with its size against its budget and a "Clear" each, and "Clear
/// caches" for all of them; both ask first.
fn caches_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::caches::{CLEAR_COMMAND, STATS_COMMAND};
    s.status(Status::command(
        "cacheUse",
        &t(Message::new("gui-caches-use", "In use")),
        STATS_COMMAND,
        "/summary",
    ));
    s.action(
        Action::command(CLEAR_COMMAND)
            .title(&t(Message::new("gui-caches-clear-all", "Clear caches")))
            .confirmation(Confirmation::new(
                &t(Message::new("gui-caches-clear-all-title", "Clear every cache?")),
                &t(Message::new(
                    "gui-caches-clear-all-ask",
                    "Thumbnails are read again from the files and images rendered again when shown: slower for a while. Photos, edits and the catalogue are not touched.",
                )),
                &t(Message::new("gui-caches-clear", "Clear")),
            )),
    );
    s.list(
        Records::command("caches", "", STATS_COMMAND, "/kinds", "id")
            .titles("title", Some("detail"))
            .column("sizeText", "")
            .row_action(
                Action::command(CLEAR_COMMAND)
                    .title(&t(Message::new("gui-caches-clear", "Clear")))
                    .args(json!({ "kind": "${row.kind}" })),
            )
            .empty(
                &t(Message::new(
                    "gui-caches-none",
                    "No cache holds anything yet.",
                )),
                None,
            ),
    );
}

/// Settings > Accounts: every import account with its service, the saved
/// sources using it, when it was last used and its status (connected,
/// needs reconnecting, access refused); its name edited in place, and
/// "Reconnect" and "Disconnect" on each, Disconnect asking what to do
/// with the sources using it (`import.accounts`, `import.renameAccount`,
/// `import.reconnect`, `import.disconnect`); "Add account…" per service
/// (`import.accountConnectors`, `import.addSource`).
fn accounts_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::shell::{
        ACCOUNT_CONNECTORS_COMMAND, ACCOUNTS_COMMAND, ADD_SOURCE_COMMAND, DISCONNECT_COMMAND,
        RECONNECT_COMMAND, RENAME_COMMAND,
    };
    s.description(&t(Message::new(
        "gui-settings-accounts-about",
        "The accounts Helios imports from. Their keys and tokens stay in this computer's keyring; the library keeps only their names.",
    )));
    let account = json!({ "account": "${row.id}" });
    let disconnect = |sources: Option<&str>| {
        let mut args = account.clone();
        if let Some(s) = sources {
            args["sources"] = json!(s);
        }
        Action::command(DISCONNECT_COMMAND).args(args)
    };
    let title = t(
        Message::new("gui-account-disconnect-title", "Disconnect { $account }?")
            .with("account", "${row.label}"),
    );
    let table = Records::command("accounts", "", ACCOUNTS_COMMAND, "/accounts", "id")
        .column(
            "label",
            &t(Message::new("gui-settings-account-name", "Account")),
        )
        .column(
            "connectorText",
            &t(Message::new("gui-settings-account-service", "Service")),
        )
        .column(
            "usedBy",
            &t(Message::new("gui-settings-account-used-by", "Used by")),
        )
        .column(
            "lastUsedText",
            &t(Message::new("gui-settings-account-last-used", "Last used")),
        )
        .column(
            "statusText",
            &t(Message::new("gui-settings-account-status", "Status")),
        )
        .column("why", &t(Message::new("gui-settings-account-why", "Why")))
        .tone("tone")
        .row_field(
            RowField::text(
                "name",
                &t(Message::new("gui-settings-account-rename", "Name")),
                RENAME_COMMAND,
                json!({ "account": "${row.id}", "name": "${value}" }),
            )
            .help(&t(Message::new(
                "gui-settings-account-rename-help",
                "How this account is called in Helios; the address it signed in as stays beside it.",
            ))),
        )
        .row_action(
            Action::command(RECONNECT_COMMAND)
                .title(&t(Message::new("gui-account-reconnect", "Reconnect")))
                .args(account.clone()),
        )
        .row_action(
            disconnect(None)
                .title(&t(Message::new("gui-account-disconnect", "Disconnect")))
                .when("!row.used")
                .danger()
                .confirmation(
                    Confirmation::new(
                        &title,
                        &t(Message::new(
                            "gui-account-disconnect-explain",
                            "Helios asks the service to revoke its access, forgets this account's keys and tokens on this computer and removes the account. What was imported stays.",
                        )),
                        &t(Message::new("gui-account-disconnect", "Disconnect")),
                    )
                    .danger(),
                ),
        )
        .row_action(
            disconnect(Some("keep"))
                .title(&t(Message::new(
                    "gui-account-disconnect-keep",
                    "Disconnect, keep its sources",
                )))
                .when("row.used")
                .confirmation(Confirmation::new(
                    &title,
                    &t(Message::new(
                        "gui-account-disconnect-keep-explain",
                        "Helios asks the service to revoke its access and forgets this account's keys and tokens on this computer. These sources stay, not connected, until you reconnect: { $sources }.",
                    )
                    .with("sources", "${row.usedBy}")),
                    &t(Message::new(
                        "gui-account-disconnect-keep",
                        "Disconnect, keep its sources",
                    )),
                )),
        )
        .row_action(
            disconnect(Some("forget"))
                .title(&t(Message::new(
                    "gui-account-disconnect-forget",
                    "Disconnect and forget its sources",
                )))
                .when("row.used")
                .danger()
                .confirmation(
                    Confirmation::new(
                        &title,
                        &t(Message::new(
                            "gui-account-disconnect-forget-explain",
                            "Helios asks the service to revoke its access, forgets this account's keys and tokens on this computer, removes the account and forgets these sources: { $sources }. What was imported stays.",
                        )
                        .with("sources", "${row.usedBy}")),
                        &t(Message::new(
                            "gui-account-disconnect-forget",
                            "Disconnect and forget its sources",
                        )),
                    )
                    .danger(),
                ),
        )
        .empty(
            &t(Message::new(
                "gui-settings-accounts-none",
                "No account yet: add one below.",
            )),
            None,
        );
    s.table(table);
    let add = Records::command(
        "account-services",
        &t(Message::new("gui-settings-accounts-add", "Add an account")),
        ACCOUNT_CONNECTORS_COMMAND,
        "/connectors",
        "connector",
    )
    .column(
        "connectorText",
        &t(Message::new("gui-settings-account-service", "Service")),
    )
    .row_action(
        Action::command(ADD_SOURCE_COMMAND)
            .title(&t(Message::new("gui-account-add", "Add account…")))
            .args(json!({ "connector": "${row.connector}" })),
    )
    .empty(
        &t(Message::new(
            "gui-settings-accounts-no-service",
            "No service with accounts is installed.",
        )),
        None,
    );
    s.table(add);
}

/// Settings > Accounts' "Use my own OAuth client": each service signing
/// in through OAuth, the client its sign-ins use (Helios's, the
/// environment's or the person's own), the person's own client id and
/// secret typed in place (kept in the keyring, the secret never shown
/// again), and "Use Helios's client" (`import.oauthClients`,
/// `import.setOAuthClient`).
fn clients_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::shell::{OAUTH_CLIENTS_COMMAND, SET_OAUTH_CLIENT_COMMAND};
    s.description(&t(Message::new(
        "gui-settings-clients-about",
        "Helios signs in to these services with its own OAuth client. To use yours instead (a desktop application you registered with the service), give its client ID and secret: they stay in this computer's keyring. Sign in again afterwards.",
    )));
    let table = Records::command(
        "oauth-clients",
        "",
        OAUTH_CLIENTS_COMMAND,
        "/clients",
        "connector",
    )
    .column(
        "connectorText",
        &t(Message::new("gui-settings-account-source", "Source")),
    )
    .column(
        "sourceText",
        &t(Message::new("gui-settings-client-used", "Client used")),
    )
    .tone("tone")
    .row_field(RowField::text(
        "clientId",
        &t(Message::new("gui-settings-client-id", "Your client ID")),
        SET_OAUTH_CLIENT_COMMAND,
        json!({ "connector": "${row.connector}", "clientId": "${value}" }),
    ))
    .row_field(
        RowField::text(
            "clientSecret",
            &t(Message::new(
                "gui-settings-client-secret",
                "Your client secret",
            )),
            SET_OAUTH_CLIENT_COMMAND,
            json!({ "connector": "${row.connector}", "clientSecret": "${value}" }),
        )
        .help(&t(Message::new(
            "gui-settings-client-secret-help",
            "Kept in the keyring and never shown again: type it again to change it.",
        ))),
    )
    .row_action(
        Action::command(SET_OAUTH_CLIENT_COMMAND)
            .title(&t(Message::new(
                "gui-settings-client-helios",
                "Use Helios's client",
            )))
            .args(json!({ "connector": "${row.connector}", "own": false }))
            .when("row.own"),
    )
    .empty(
        &t(Message::new(
            "gui-settings-clients-none",
            "No service signing in through OAuth is installed.",
        )),
        None,
    );
    s.table(table);
}

/// The screens (`helios.display.screens`): each with what the system
/// says it shows and the person's correction (`auto` follows the
/// system), written by `helios.display.screen_override`. A screen not
/// connected stays listed while it has a correction.
fn screens_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::display::ScreenColour as C;
    use crate::display::{OVERRIDES, SCREEN_OVERRIDE_COMMAND, SCREENS_COMMAND, colour_text};
    // In `OVERRIDES`' order: auto, srgb, wide-gamut, hdr.
    let labels = [
        t(Message::new(
            "gui-display-screen-auto",
            "Auto (what the system says)",
        )),
        t(colour_text(Some(C::Srgb))),
        t(colour_text(Some(C::WideGamut))),
        t(colour_text(Some(C::Hdr))),
    ];
    let choices: Vec<(Json, &str)> = OVERRIDES
        .iter()
        .zip(&labels)
        .map(|(v, l)| (json!(v), l.as_str()))
        .collect();
    let table = Records::command(
        "screens",
        &t(Message::new("gui-settings-display-screens", "Screens")),
        SCREENS_COMMAND,
        "/screens",
        "name",
    )
    .column(
        "name",
        &t(Message::new("gui-settings-display-screen", "Screen")),
    )
    .column(
        "systemText",
        &t(Message::new(
            "gui-settings-display-system-says",
            "The system says",
        )),
    )
    .row_field(
        RowField::choice(
            "override",
            &t(Message::new("gui-settings-display-shows", "Shows")),
            &choices,
            SCREEN_OVERRIDE_COMMAND,
            json!({ "screen": "${row.name}", "value": "${value}" }),
        )
        .help(&t(Message::new(
            "gui-settings-display-shows-help",
            "Corrects what the system says this screen shows; Auto follows the system. HDR is detected and shown only: an HDR screen is drawn as wide gamut for now.",
        ))),
    )
    .empty(
        &t(Message::new(
            "gui-settings-display-no-screens",
            "No screen known yet.",
        )),
        None,
    );
    s.table(table);
}

/// The catalogue's snapshots (`library.catalogue.snapshots`), newest
/// first, each with "Restore…" (asked first), and "Snapshot now".
fn snapshots_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::catalogue::{LIST_COMMAND, RESTORE_COMMAND, SNAPSHOT_COMMAND};
    // Only with a library open: there is no catalogue to snapshot
    // without one.
    s.action(
        Action::command(SNAPSHOT_COMMAND)
            .title(&t(Message::new(
                "gui-catalogue-snapshot-now",
                "Snapshot now",
            )))
            .when("libraryOpen"),
    );
    s.list(
        Records::command(
            "snapshots",
            &t(Message::new("gui-settings-snapshots", "Snapshots")),
            LIST_COMMAND,
            "",
            "file",
        )
        .titles("whenText", Some("placeText"))
        .column("reason", "")
        .row_action(
            Action::command(RESTORE_COMMAND)
                .title(&t(Message::new("gui-catalogue-restore", "Restore…")))
                .args(json!({ "file": "${row.file}" }))
                .confirmation(
                    Confirmation::new(
                        &t(Message::new(
                            "gui-catalogue-restore-title",
                            "Restore the catalogue?",
                        )),
                        &t(Message::new(
                            "gui-catalogue-restore-ask",
                            "Restore the catalogue from the snapshot of { $when }? Changes made since are lost. The current catalogue is kept aside, and Helios closes and starts again on the snapshot.",
                        )
                        .with("when", "${row.whenText}")),
                        &t(Message::new(
                            "gui-catalogue-restore-confirm",
                            "Restore and restart",
                        )),
                    )
                    .danger(),
                ),
        )
        .empty(
            &t(Message::new("gui-catalogue-none", "No snapshot yet.")),
            None,
        )
        .when("libraryOpen"),
    );
}

/// The plugin: declares the settings and the manifest, follows their
/// changes. Values come once the desktop [`install`]s them; until then
/// the settings show their defaults and nothing is written.
#[derive(Default)]
pub struct BridgePlugin {
    /// The interface's catalogues, for the manifest's texts; English
    /// without.
    pub i18n: Option<egui_pop::i18n::I18n>,
    /// The plugins installed, their settings declared and placed under
    /// Plugins, as known when the application is built.
    pub plugins: plugins::Shared,
}

impl std::fmt::Debug for BridgePlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgePlugin").finish_non_exhaustive()
    }
}

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
        let i18n = self.i18n.clone();
        let t = |m: Message| match &i18n {
            Some(i) => helios_i18n::message(i, &m),
            None => m.english,
        };
        let installed = lock(&self.plugins).clone();
        plugins::declare(ctx, &installed, &t);
        for scope in SCOPES {
            admin.declare_scope(scope);
        }
        admin.manifest(manifest(&schema, &installed, &t));
        let words = self.i18n.clone();
        admin.hook(
            crate::shell::init_wizard::CHECK_FOLDER,
            check_library_folder(move |m| match &words {
                Some(i) => helios_i18n::message(i, &m),
                None => m.english,
            }),
        );
        let words = self.i18n.clone();
        admin.hook(
            crate::reset::CHECK_WORD,
            crate::reset::check_word(move |m| match &words {
                Some(i) => helios_i18n::message(i, &m),
                None => m.english,
            }),
        );
        let words = self.i18n.clone();
        admin.hook(
            crate::forget::CHECK_WORD,
            crate::forget::check_word(move |m| match &words {
                Some(i) => helios_i18n::message(i, &m),
                None => m.english,
            }),
        );
        admin.set_foci(focus::Foci);
        ctx.service(
            &BRIDGE,
            Bridge {
                plugins: installed,
                ..Bridge::default()
            },
        )?;
        Ok(())
    }
}

fn setting_id(key: &str) -> Result<SettingId, PluginError> {
    SettingId::new(key).map_err(|e| PluginError::Failed(format!("{key}: {e:?}")))
}

/// Hands the resolved settings to the Settings app, in a store
/// ([`store::composite`]): Helios's settings kept through `node` (the
/// person's and the library's layers; none without a vault) and the
/// machine's file in `config_dir`, the interface's own in a file there,
/// the open workspace's in its file. The library's value scope is
/// offered when a library is open, and `--set` shown as the command
/// line's. The node's layers are followed live by [`follow`].
pub fn install(
    app: &AppHandle,
    settings: Settings,
    node: Option<Arc<dyn NodeLayers>>,
    config_dir: &Path,
) -> NodeSettingsStore {
    let schema = Schema::core();
    let project = settings.project().is_some();
    let library = settings.project().map(focus::library_name);
    let run: Vec<(&'static str, Json)> = schema
        .iter()
        .filter_map(|s| {
            let v = settings.config().value_in(Layer::Run, s.key)?;
            Some((s.key, to_json(&s.kind, &v)))
        })
        .collect();
    let store = NodeSettingsStore::new(settings, node, config_dir);
    let (Some(bridge), Some(admin)) = (app.service(&BRIDGE), app.service(&ADMIN)) else {
        return store;
    };
    admin.offer(LIBRARY.scope, project);
    for (key, v) in run {
        if let Ok(id) = SettingId::new(key)
            && let Err(e) = admin.set_run(app, &id, v)
        {
            log::warn!("--set {key}: {e}");
        }
    }
    {
        let mut st = lock(&bridge.state);
        let secrets = st
            .secrets
            .clone()
            .unwrap_or_else(|| Arc::new(helios_connect::keyring::SystemSecrets));
        st.plugins = Some(plugins::PluginSettingsStore::new(
            &bridge.plugins,
            &store,
            config_dir,
            secrets,
        ));
        st.installed = Some((store.clone(), config_dir.to_path_buf()));
        st.library = library;
    }
    attach(app, &bridge, &admin);
    store
}

/// Hands the plugins their settings' values: `broker` reads them from
/// the plugins' store [`install`] made as each plugin starts, and is told
/// when they change ([`plugins::connect`]). Nothing without one.
pub fn connect_plugins(app: &AppHandle, broker: &Arc<helios_plugin::Broker>) {
    let Some(bridge) = app.service(&BRIDGE) else {
        return;
    };
    let store = lock(&bridge.state).plugins.clone();
    if let Some(store) = store {
        plugins::connect(&store, broker, &*app.bus());
    }
}

/// The listener of the local node's layers changed
/// ([`listen_layers`]).
pub const LAYERS_LISTENER: &str = "helios.settings-layers";

/// The owner of the settings' listeners on the application's bus.
pub const BUS_OWNER: &str = "helios.settings";

/// Listens, as [`LAYERS_LISTENER`], to the layers the desktop's node says
/// changed (`helios.settings.layerChanged`, on the application's bus `bus`); a
/// remote node's (`relayed:<name>`) are its own layers, left out. Each
/// layer's name is kept in the receiver until [`follow`] reads it:
/// registered before the settings are read, no change made between the
/// read and the shell's build is missed.
pub fn listen_layers(
    bus: &dyn egui_pop::api::bus::bus_contract::Bus,
) -> Result<std::sync::mpsc::Receiver<String>, egui_pop::api::bus::bus_contract::Refused> {
    use helios_core::FactPoint;
    use helios_node_core::settings::LayerChanged;
    let (tx, rx) = std::sync::mpsc::channel();
    bus.listen::<FactPoint<LayerChanged>>(
        LAYERS_LISTENER,
        egui_pop::api::bus::bus_contract::ListenOpts::new().owner(BUS_OWNER),
        move |fact, ()| {
            if !fact.source.starts_with(crate::relay::RELAYED)
                && let Some(name) = fact.payload["layer"].as_str()
            {
                let _ = tx.send(name.to_string());
            }
            Ok(())
        },
    )?;
    Ok(rx)
}

/// Follows the layers the node says changed (`heard`, from
/// [`listen_layers`]) for every store [`install`] made that keeps values
/// in the node's layers: Helios's settings and the plugins'. A layer
/// another client changes is read again by each, and what moved reaches
/// the Settings app, so every `settings.changed` listener hears it. One
/// listener for all of them, so none is left out. After [`install`].
pub fn follow(app: &AppHandle, heard: std::sync::mpsc::Receiver<String>) {
    let Some(bridge) = app.service(&BRIDGE) else {
        return;
    };
    let (helios, plugins) = {
        let st = lock(&bridge.state);
        let helios = st.installed.as_ref().map(|(s, _)| s.clone());
        (helios, st.plugins.clone())
    };
    if helios.is_none() && plugins.is_none() {
        log::warn!("settings: the node's layers are followed before they are installed");
        return;
    }
    store::on_layers_changed(heard, move |name| {
        if let Some(s) = &helios {
            s.layer_changed(name);
        }
        if let Some(s) = &plugins {
            s.layer_changed(name);
        }
    });
}

/// Gives the Settings app the store for what is open now. A value the
/// workspace's file holds for a setting that does not opt into the
/// workspace scope (one made app-wide since it was saved) moves to the
/// app's value, with a note in the log, so nobody silently loses a
/// setting; the file is saved without it.
fn attach(app: &AppHandle, bridge: &Bridge, admin: &Admin) {
    let st = lock(&bridge.state);
    let Some((node, config_dir)) = &st.installed else {
        return;
    };
    // The import window's focus has no workspace.
    let workspace = st.workspace.as_deref().filter(|_| !st.import_edited);
    let store = store::composite(node, st.plugins.as_ref(), config_dir, workspace);
    if let Some(file) = workspace {
        let values: BTreeMap<String, Json> = std::fs::read_to_string(file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let (stray, kept): (BTreeMap<_, _>, BTreeMap<_, _>) = values
            .into_iter()
            .partition(|(k, _)| stray(app, admin, WORKSPACE.scope, k));
        if !stray.is_empty() {
            let to_app = stray.iter().map(|(k, v)| (k.clone(), Some(v.clone())));
            // The file is written whole: the composite would route a
            // Helios setting's removal to a store without this scope.
            let saved = store
                .save(ValueScope::App, &to_app.collect())
                .and_then(|()| {
                    let text = serde_json::to_string_pretty(&kept).map_err(|e| e.to_string())?;
                    std::fs::write(file, text + "\n")
                        .map_err(|e| format!("{}: {e}", file.display()))
                });
            match saved {
                Ok(()) => {
                    for key in stray.keys() {
                        log::info!(
                            "settings: `{key}` is no longer set per workspace: its value there moved to the app's"
                        );
                    }
                }
                Err(e) => log::warn!("settings: the workspace's values not moved: {e}"),
            }
        }
    }
    drop(st);
    admin.attach_store(store);
}

/// Whether a known setting does not opt into a scope.
fn stray(app: &AppHandle, admin: &Admin, scope: ValueScope, key: &str) -> bool {
    let Ok(id) = SettingId::new(key) else {
        return false;
    };
    let known = egui_pop::core::lock(&app.store()).get(&id.path()).is_some();
    known && !admin.allows(app, &id, scope)
}

/// Every setting's `app` value, as the Settings app holds it.
fn app_values(settings: &Settings) -> BTreeMap<&'static str, Json> {
    Schema::core()
        .iter()
        .filter_map(|s| Some((s.key, to_json(&s.kind, &settings.app_value(s.key)?))))
        .collect()
}

/// The project settings' values in the library's layer (the Settings
/// app's `library` value scope).
fn library_values(settings: &Settings) -> BTreeMap<String, Json> {
    Schema::core()
        .iter()
        .filter(|s| s.concern == Concern::Project)
        .filter_map(|s| {
            let v = settings.config().value_in(Layer::Project, s.key)?;
            Some((s.key.to_string(), to_json(&s.kind, &v)))
        })
        .collect()
}

/// Opens a workspace's own values of the settings (the `workspace` value
/// scope), kept in `file`; `None` closes it.
pub fn open_workspace(app: &AppHandle, file: Option<PathBuf>) {
    let (Some(bridge), Some(admin)) = (app.service(&BRIDGE), app.service(&ADMIN)) else {
        return;
    };
    let import_edited = {
        let mut st = lock(&bridge.state);
        st.workspace = file;
        st.import_edited
    };
    let offered = lock(&bridge.state).workspace.is_some() && !import_edited;
    admin.offer(WORKSPACE.scope, offered);
    attach(app, &bridge, &admin);
}

/// The open workspace's name, which the Settings header names the main
/// window's focus by (`None`: no workspace).
pub fn name_workspace(app: &AppHandle, name: Option<&str>) {
    if let Some(bridge) = app.service(&BRIDGE) {
        lock(&bridge.state).workspace_name = name.filter(|n| !n.is_empty()).map(String::from);
    }
}

/// The Settings edit another focus: its scopes offered, its store
/// attached ([`focus`]).
fn reattach(app: &AppHandle) {
    let (Some(bridge), Some(admin)) = (app.service(&BRIDGE), app.service(&ADMIN)) else {
        return;
    };
    let offered = {
        let st = lock(&bridge.state);
        st.workspace.is_some() && !st.import_edited
    };
    admin.offer(WORKSPACE.scope, offered);
    attach(app, &bridge, &admin);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A partial run lists its own steps: a step out of it is hidden by
    /// its skip value, the others stay; with none, every step shows.
    #[test]
    fn the_setup_shows_the_steps_of_its_run_alone() {
        use egui_pop::api::admin::model::Flow;
        use egui_pop::model::when::WhenContext;
        use helios_init::{Registry, Step};
        let mut r = Registry::default();
        for id in ["a.one", "a.two"] {
            let step = Step::new(id, Message::plain(id), Message::plain(id)).fills(&[id]);
            r.add("a", step).unwrap();
        }
        let w = init_wizard(&r, &|m| m.english);
        let base = WhenContext::new();
        let shown = |values| {
            Flow::start(&w, &base, values)
                .shown(&w, &base)
                .iter()
                .map(|s| s.meta.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(std::collections::BTreeMap::default()),
            ["a.one", "a.two"]
        );
        let skip = crate::shell::init_wizard::skip_key("a.one");
        assert_eq!(shown([(skip, serde_json::json!(true))].into()), ["a.two"]);
    }

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
        let m = manifest(&schema, &[], &|m| m.english);
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
        assert_eq!(decisions, Some(vec![LIBRARY.scope]));
        // Every descriptor reads its own default back.
        for s in schema.iter() {
            let d = descriptor(s);
            let j = to_json(&s.kind, &s.kind.default_value());
            assert_eq!(d.default.to_json(), j, "{}", s.key);
            assert_eq!(from_json(&s.kind, &j), Some(s.kind.default_value()));
        }
    }

    #[test]
    fn the_manifest_s_texts_go_through_the_catalogues() {
        let m = manifest(&Schema::core(), &[], &|m| format!("<{}>", m.key));
        let library = m.scopes.iter().find(|s| s.meta.id == "library").unwrap();
        assert_eq!(library.meta.title, "<gui-settings-library>");
        for s in &m.sections {
            assert!(
                s.meta.title.starts_with("<gui-settings-"),
                "{}/{}: {}",
                s.scope,
                s.meta.id,
                s.meta.title
            );
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
