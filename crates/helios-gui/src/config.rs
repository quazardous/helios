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

use egui_pop::api::admin::model::{
    Action, Clauses as _, Confirmation, FORMAT, Field, FieldKind, Manifest, Records, RowField,
    SectionBuilder, Status, ValueScope, Wizard,
};
use egui_pop::api::admin::{ADMIN, Admin};
use egui_pop::api::hooks::On;
use egui_pop::api::services::ServiceToken;
use egui_pop::api::{AppHandle, Ctx, Plugin, PluginError};
use egui_pop::model::commands::Origin;
use egui_pop::model::ids::{OwnerId, PluginId, SettingId};
use egui_pop::model::values::{Change, Descriptor, Set, Value as PopValue};
use helios_config::{Concern, Config, Kind, Layer, Schema, Value};
use helios_core::Message;
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
    ("library/decisions", &["project.decisions.mode"]),
    ("library/history", &["system.workflow.keep_days"]),
    (
        "storage/caches",
        &[
            "system.cache.budget",
            "system.cache.share",
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
        "interface/workbench" => Message::new("gui-settings-workbench", "Workbench"),
        "interface/experimental" => Message::new("gui-settings-experimental", "Experimental"),
        "library/image-map" => Message::new("gui-settings-image-map", "Image map"),
        "library/decisions" => Message::new("gui-settings-decisions", "Decisions"),
        "library/history" => Message::new("gui-settings-history", "History"),
        "storage/volumes" => Message::new("gui-settings-volumes", "Volumes"),
        "storage/caches" => Message::new("gui-settings-caches", "Caches"),
        "storage/catalogue" => Message::new("gui-settings-catalogue", "Catalogue"),
        "hardware/workers" => Message::new("gui-settings-workers", "Workers"),
        "hardware/display" => Message::new("gui-settings-display", "Display"),
        "backup/strategy" => Message::new("gui-settings-strategy", "Strategy"),
        "backup/destinations" => Message::new("gui-settings-destinations", "Destinations"),
        other => Message::from(other.to_string()),
    }
}

/// The manifest: Helios's scopes, in the note's order, and its settings
/// placed in them (the display's, the catalogue's and the backup
/// destinations' egui-pop settings too), with the Storage and Backup
/// pages read from the storage commands; its texts through `t` (the
/// interface's language).
pub fn manifest(schema: &Schema, t: &dyn Fn(Message) -> String) -> Manifest {
    let tt = |path: &str| t(title(path));
    let mut m = Manifest::new(FORMAT)
        .scope_at("library", &tt("library"), "folder", 30)
        .scope_at("storage", &tt("storage"), "disc", 40)
        .scope_at("backup", &tt("backup"), "lock", 50)
        .scope_at("hardware", &tt("hardware"), "command", 70)
        .scope_at("health", &tt("health"), "success", 90);
    m = m.section("storage/volumes", &tt("storage/volumes"), |s| {
        volumes_section(s, t);
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
    m.section("storage/catalogue", &tt("storage/catalogue"), |s| {
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
    })
    .section("backup/strategy", &tt("backup/strategy"), |s| {
        strategy_section(s, t);
    })
    .section("backup/destinations", &tt("backup/destinations"), |s| {
        s.field(Field::setting(
            crate::platform::SETTING_DESTINATION_DAYS.as_str(),
        ));
    })
    .section("health/check", &tt("health/check"), |s| {
        health_section(s, t);
    })
    .wizard(backup_wizard(t))
}

/// The backup wizard's id: "Set up backups…" on the Backup scope's
/// strategy opens it.
pub const BACKUP_WIZARD: &str = "backup.setup";

/// The backup wizard: how many copies, on which disks, one off site
/// (asked only for two copies or more), when they are made; a summary,
/// then `library.setUpBackups` with the choices.
fn backup_wizard(t: &dyn Fn(Message) -> String) -> Wizard {
    use crate::shell::SET_UP_STRATEGY_COMMAND;
    let field = |key: &str, title: Message, kind| Field::draft(BACKUP_WIZARD, key, &t(title), kind);
    let copies = (1..=5).fold(
        field(
            "copies",
            Message::new("gui-backup-wizard-copies", "Copies of each photo"),
            FieldKind::Choice,
        ),
        |f, n| f.choice(json!(n), &n.to_string()),
    );
    Wizard::new(
        BACKUP_WIZARD,
        &t(Message::new("gui-backup-wizard", "Set up backups")),
        SET_UP_STRATEGY_COMMAND,
    )
    .description(&t(Message::new(
        "gui-backup-wizard-about",
        "Helios keeps verified copies of every photo on your backup disks: choose how many, where, and when.",
    )))
    .step(
        "copies",
        &t(Message::new("gui-backup-wizard-step-copies", "Copies")),
        |s| {
            s.description(&t(Message::new(
                "gui-backup-wizard-copies-about",
                "Each copy is on a different disk, besides the photo in the library. Two copies survive a disk failing.",
            )))
            .field(copies.default_value(json!(2)));
        },
    )
    .step(
        "destination",
        &t(Message::new("gui-backup-wizard-step-destination", "Disks")),
        |s| {
            s.field(
                field(
                    "destination",
                    Message::new("gui-backup-wizard-destination", "Copy to"),
                    FieldKind::Choice,
                )
                .choice(
                    json!("backup"),
                    &t(Message::new(
                        "gui-backup-wizard-destination-backup",
                        "Any disk with the backup role",
                    )),
                )
                .choice(
                    json!("archive"),
                    &t(Message::new(
                        "gui-backup-wizard-destination-archive",
                        "Any disk with the archive role",
                    )),
                )
                .required()
                .help(&t(Message::new(
                    "gui-backup-wizard-destination-help",
                    "Give a disk its role on Storage › Volumes; one disk per copy.",
                ))),
            );
        },
    )
    .step(
        "off-site",
        &t(Message::new("gui-backup-wizard-step-off-site", "Off site")),
        |s| {
            s.field(
                field(
                    "offSite",
                    Message::new(
                        "gui-backup-wizard-off-site",
                        "Keep one of the copies off site",
                    ),
                    FieldKind::Toggle,
                )
                .default_value(json!(false))
                .help(&t(Message::new(
                    "gui-backup-wizard-off-site-help",
                    "A disk kept elsewhere survives a fire or a theft: mark it off site on Storage › Volumes.",
                ))),
            );
        },
    )
    .when("form.copies != '1'")
    .step(
        "when",
        &t(Message::new("gui-backup-wizard-step-when", "When")),
        |s| {
            s.description(&t(Message::new(
                "gui-backup-wizard-when-about",
                "Back up now on the Backup page makes copies at any time.",
            )))
            .field(
                field(
                    "onImport",
                    Message::new("gui-backup-wizard-on-import", "After each import"),
                    FieldKind::Toggle,
                )
                .default_value(json!(true)),
            )
            .field(
                field(
                    "onConnect",
                    Message::new(
                        "gui-backup-wizard-on-connect",
                        "When a backup disk is plugged in",
                    ),
                    FieldKind::Toggle,
                )
                .default_value(json!(true)),
            );
        },
    )
    .verb(&t(Message::new("gui-backup-wizard-finish", "Use this strategy")))
}

/// The Health scope: how it stands, "Check now", and the findings of the
/// last run (`health.findings`), the worst first, toned by their level,
/// each with its fix ("Fix…", asked first when it changes something not
/// easily taken back) and "Ignore"; the ignored ones last, with "Show
/// again".
fn health_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::shell::{HEALTH_CHECK_NOW, HEALTH_FINDINGS, HEALTH_FIX, HEALTH_IGNORE};
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
    s.list(
        Records::command("findings", "", HEALTH_FINDINGS, "/findings", "id")
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
            .empty(
                &t(Message::new(
                    "gui-health-not-yet",
                    "Not checked yet: the checks run in the background.",
                )),
                None,
            ),
    );
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

/// The Storage scope's volume table: every volume known
/// (`library.volumes`), toned by its state: its name edited in place, a
/// toggle per role, the off-site and RAID flags, its space as a meter,
/// when and where it was last seen, and its actions ("Mark this disk…"
/// and "Forget…" ask first).
fn volumes_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::shell::{
        FORGET_VOLUME, MARK_VOLUME_COMMAND, VOLUME_ROLE_COMMAND, VOLUME_SET_COMMAND,
        VOLUMES_COMMAND,
    };
    use helios_storage::Role;
    s.description(&t(Message::new(
        "gui-storage-intro",
        "The disks Helios has seen. Name them and say what each is for: Helios writes only to library, archive, backup and scratch disks.",
    )));
    s.status(Status::command(
        "volumes-summary",
        &t(Message::new("gui-settings-volumes-seen", "Volumes")),
        VOLUMES_COMMAND,
        "/summary",
    ));
    let volume = json!({ "volume": "${row.id}" });
    let set = |key: &str| json!({ "volume": "${row.id}", key: "${value}" });
    let mut table = Records::command("volumes", "", VOLUMES_COMMAND, "/volumes", "id")
        .column("name", &t(Message::new("gui-settings-volume-name", "Name")))
        .column(
            "stateText",
            &t(Message::new("gui-settings-volume-state", "State")),
        )
        .column("kind", &t(Message::new("gui-settings-volume-kind", "Kind")))
        .row_field(
            RowField::text(
                "name",
                &t(Message::new("gui-settings-volume-name", "Name")),
                VOLUME_SET_COMMAND,
                set("name"),
            )
            .help(&t(Message::new(
                "gui-storage-name-hint",
                "Its name; Enter saves it",
            ))),
        );
    for role in Role::ALL {
        let name = serde_json::to_value(role)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        table = table.row_field(RowField::toggle(
            &format!("role_{name}"),
            &t(crate::shell::role_text(role)),
            VOLUME_ROLE_COMMAND,
            json!({ "volume": "${row.id}", "role": name, "on": "${value}" }),
        ));
    }
    let table = table
        .row_field(
            RowField::toggle(
                "offSite",
                &t(Message::new("gui-storage-off-site", "Off site")),
                VOLUME_SET_COMMAND,
                set("offSite"),
            )
            .help(&t(Message::new(
                "gui-storage-off-site-hint",
                "Kept away from the other disks (another building)",
            ))),
        )
        .row_field(
            RowField::toggle(
                "raid",
                &t(Message::new("gui-storage-raid", "RAID")),
                VOLUME_SET_COMMAND,
                set("raid"),
            )
            .help(&t(Message::new(
                "gui-storage-raid-note",
                "A RAID counts as one copy: it survives a dead disk, not a deletion or a theft.",
            ))),
        )
        .meter(
            "used",
            &t(Message::new("gui-settings-volume-space", "Space")),
            "space",
        )
        .column(
            "photos",
            &t(Message::new("gui-settings-volume-photos", "Photos")),
        )
        .column(
            "backupText",
            &t(Message::new("gui-settings-volume-backup", "Backup")),
        )
        .column(
            "seen",
            &t(Message::new("gui-settings-volume-seen", "Seen")),
        )
        .tone("tone")
        .row_action(
            Action::command(MARK_VOLUME_COMMAND)
                .title(&t(Message::new("gui-storage-mark", "Mark this disk…")))
                .args(volume.clone())
                .when("row.weak")
                .confirmation(Confirmation::new(
                    &t(Message::new(
                        "gui-storage-mark-title",
                        "Mark { $disk }?",
                    )
                    .with("disk", "${row.name}")),
                    &t(Message::new(
                        "gui-storage-mark-explain",
                        "This disk is known only by its label and size, so Helios could take another disk for it. Marking it writes a small file, .helios/volume.json, at its root: nothing else on the disk changes.",
                    )),
                    &t(Message::new(
                        "gui-storage-mark-confirm",
                        "Write the identity file",
                    )),
                )),
        )
        .row_action(
            Action::command("library.map.scanNow")
                .title(&t(Message::new("gui-settings-volume-scan", "Scan now")))
                .args(volume.clone()),
        )
        .row_action(
            Action::command(FORGET_VOLUME)
                .title(&t(Message::new("gui-settings-volume-forget", "Forget…")))
                .args(volume)
                .danger()
                .confirmation(
                    Confirmation::new(
                        &t(Message::new(
                            "gui-settings-volume-forget-title",
                            "Forget what was seen on { $disk }?",
                        )
                        .with("disk", "${row.name}")),
                        &t(Message::new(
                            "gui-storage-forget-hint",
                            "Drops what the image map knows of this volume's files not in the Library; the volume stays known",
                        )),
                        &t(Message::new("gui-settings-volume-forget-verb", "Forget")),
                    )
                    .danger(),
                ),
        )
        .empty(
            &t(Message::new(
                "gui-settings-volumes-none",
                "No volume known yet: plug in a disk.",
            )),
            None,
        )
        .refresh("storage.*");
    s.table(table);
}

/// The Backup scope's strategy, edited in place: on or off, how many
/// copies, where, one off site, when copies are made (each written at
/// once through `library.setBackupStrategy`); its disks plugged in and
/// "Back up now".
fn strategy_section(s: &mut SectionBuilder<'_>, t: &dyn Fn(Message) -> String) {
    use crate::shell::{BACKUP_NOW_COMMAND, BACKUP_STRATEGY_COMMAND, SET_STRATEGY_COMMAND};
    let part = |key: &str| json!({ "key": key, "value": "${value}" });
    let toggle = |key: &str, title: Message| {
        RowField::toggle(key, &t(title), SET_STRATEGY_COMMAND, part(key)).when("row.on")
    };
    let copies: Vec<(Json, String)> = (1..=5).map(|n| (json!(n), n.to_string())).collect();
    let copies: Vec<(Json, &str)> = copies
        .iter()
        .map(|(v, l)| (v.clone(), l.as_str()))
        .collect();
    s.list(
        Records::command("strategy", "", BACKUP_STRATEGY_COMMAND, "", "set")
            .titles("summaryText", None)
            .tone("tone")
            .row_field(RowField::toggle(
                "on",
                &t(Message::new(
                    "gui-storage-backup-on",
                    "Keep verified backup copies of every photo",
                )),
                SET_STRATEGY_COMMAND,
                part("on"),
            ))
            .row_field(
                RowField::choice(
                    "copies",
                    &t(Message::new("gui-settings-strategy-copies", "Copies")),
                    &copies,
                    SET_STRATEGY_COMMAND,
                    part("copies"),
                )
                .when("row.on"),
            )
            .row_field(
                RowField::choice(
                    "destination",
                    &t(Message::new("gui-settings-strategy-destinations", "On")),
                    &[],
                    SET_STRATEGY_COMMAND,
                    part("destination"),
                )
                .choices_from("destinationChoices")
                .when("row.on"),
            )
            .row_field(toggle(
                "offSite",
                Message::new("gui-storage-backup-off-site", "One of them off site"),
            ))
            .row_field(toggle(
                "onImport",
                Message::new("gui-storage-backup-on-import", "After each import"),
            ))
            .row_field(toggle(
                "onConnect",
                Message::new(
                    "gui-storage-backup-on-connect",
                    "When a backup disk is plugged in",
                ),
            ))
            .refresh("storage.*"),
    );
    s.status(Status::command(
        "strategy-online",
        &t(Message::new(
            "gui-settings-strategy-online",
            "Disks plugged in",
        )),
        BACKUP_STRATEGY_COMMAND,
        "/online",
    ));
    s.action(
        Action::command(BACKUP_NOW_COMMAND)
            .title(&t(Message::new("gui-storage-backup-now", "Back up now")))
            .primary(),
    );
    s.action(
        Action::command(egui_pop::api::admin::CMD_WIZARD_OPEN.as_str())
            .title(&t(Message::new(
                "gui-backup-wizard-open",
                "Set up backups…",
            )))
            .args(json!({ "id": BACKUP_WIZARD })),
    );
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
        admin.manifest(manifest(&schema, &t));
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
        let m = manifest(&schema, &|m| m.english);
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
    fn the_manifest_s_texts_go_through_the_catalogues() {
        let m = manifest(&Schema::core(), &|m| format!("<{}>", m.key));
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
