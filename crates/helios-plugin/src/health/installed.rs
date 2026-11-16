//! The plugin checks: each installed plugin starts, answers its
//! handshake, and speaks interface versions this Helios supports. They
//! read the [`Plugin`]s probed once per run ([`probe`], off the
//! interface thread); one finding per plugin, `plugins.<id>`.

use std::path::{Path, PathBuf};

use helios_core::Message;
use helios_health::{Declaration, Finding, Fix, Gather, Level, Owned, Registry};
use serde_json::json;

/// The owner of the plugins' check.
pub const OWNER: &str = "plugins";

/// Adds the plugin host's check, each plugin starts and answers, to
/// `registry`, owned by [`OWNER`], over what `source` gathers.
pub fn register(registry: &mut Registry, source: &Gather<Vec<Plugin>>) {
    helios_health::add_owned(registry, OWNER, vec![helios_health::over(Plugins, source)]);
}

/// Every plugin installed, probed (started and its handshake answered,
/// then stopped), and those whose manifest was refused.
pub fn probe(
    broker: &crate::Broker,
    book: Option<&dyn helios_connect::account::AccountBook>,
) -> Vec<Plugin> {
    use crate::Probed;
    let mut out: Vec<Plugin> = broker
        .ids()
        .filter_map(|id| {
            let m = broker.manifest(id)?;
            // A source plugin is needed once its connector (its id's last
            // part) has an account; any other plugin, always.
            let needed = || {
                if !m.services.iter().any(|s| s == "import.source") {
                    return true;
                }
                let connector = id.rsplit('.').next().unwrap_or(id);
                book.and_then(|b| b.accounts().ok())
                    .is_some_and(|accounts| accounts.iter().any(|a| a.connector == connector))
            };
            let (status, log) = match broker.probe(id) {
                Ok((Probed::Answers, log)) => (Status::Answers, log),
                Ok((Probed::Disabled, log)) => (Status::Disabled, log),
                Ok((Probed::NotGranted(capability), log)) => (
                    Status::NotGranted {
                        capability,
                        needed: needed(),
                    },
                    log,
                ),
                Ok((Probed::Failed(e), log)) => (Status::Crashed(e), log),
                Err(e) => (Status::Crashed(e), Vec::new()),
            };
            Some(Plugin {
                id: id.to_string(),
                name: m.title().to_string(),
                version: m.version.clone(),
                dir: broker.dir(id).map(Path::to_path_buf).unwrap_or_default(),
                status,
                log,
            })
        })
        .collect();
    for r in broker.rejected() {
        let manifest = crate::Manifest::read(&r.dir).ok();
        let id = manifest.as_ref().map_or_else(
            || {
                r.dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            |m| m.id.clone(),
        );
        // A copy shadowed by another of the same id is not a plugin of
        // its own.
        if broker.manifest(&id).is_some() {
            continue;
        }
        out.push(Plugin {
            name: manifest
                .as_ref()
                .map_or_else(|| id.clone(), |m| m.title().to_string()),
            version: manifest.map(|m| m.version).unwrap_or_default(),
            id,
            dir: r.dir.clone(),
            status: Status::Unsupported(r.problems.clone()),
            log: Vec::new(),
        });
    }
    out
}

pub const PLUGINS: &str = "plugins";

/// The desktop's command granting a plugin a capability it declares
/// (`plugin`, `capability`).
pub const GRANT: &str = "plugin.grant";

/// How a plugin stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// It started and answered its handshake (or runs already).
    Answers,
    /// The user turned it off.
    Disabled,
    /// It needs a `capability` the user has not granted, so it was not
    /// started. `needed`: something uses it (a source plugin's connector
    /// has an account); until then, waiting is not a problem, its
    /// permission asked when a source first uses it.
    NotGranted { capability: String, needed: bool },
    /// It did not start, crashed or did not answer its handshake.
    Crashed(String),
    /// Its manifest asks for what this Helios does not support (an
    /// interface version, a Helios version): refused before running.
    Unsupported(Vec<String>),
}

/// One installed plugin, probed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    pub id: String,
    /// Its name for people (its id when it has none).
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
    pub status: Status,
    /// Its last lines of log (standard error), the newest last.
    pub log: Vec<String>,
}

/// Each plugin's finding: answers is fine, disabled is said, not granted
/// is fine while nothing uses it and a warning (with "Allow") once
/// something does, crashed or unsupported a problem.
#[derive(Clone, Copy, Debug, Default)]
pub struct Plugins;

fn open_folder(p: &Plugin, label: Message) -> Fix {
    Fix {
        command: "library.openFolder".into(),
        args: json!({ "path": p.dir.display().to_string() }),
        label,
    }
}

/// The plugin's log, for a detail line ("" without one).
fn log_of(p: &Plugin) -> String {
    let tail: Vec<&str> = p
        .log
        .iter()
        .rev()
        .take(5)
        .rev()
        .map(String::as_str)
        .collect();
    tail.join(" / ")
}

impl Owned for Plugins {
    type State = Vec<Plugin>;

    fn id(&self) -> &str {
        PLUGINS
    }

    fn title(&self) -> Message {
        Message::new("health-plugins", "Plugins")
    }

    fn declaration(&self) -> Declaration {
        Declaration::new("Each plugin installed starts and answers its handshake")
            .levels(&[Level::Ok, Level::Warning, Level::Problem])
            .fixes(&["library.openFolder", "plugin.disable", GRANT])
    }

    fn findings(&self, state: &Vec<Plugin>) -> Vec<Finding> {
        state
            .iter()
            .map(|p| {
                let name = || p.name.clone();
                let (level, message, detail, fix) = match &p.status {
                    Status::Answers => (
                        Level::Ok,
                        Message::new("health-plugin-ok", "{ $plugin } answers")
                            .with("plugin", name()),
                        Message::new(
                            "health-plugin-ok-detail",
                            "Version { $version }, in { $dir }",
                        )
                        .with("version", p.version.clone())
                        .with("dir", p.dir.display().to_string()),
                        None,
                    ),
                    Status::Disabled => (
                        Level::Ok,
                        Message::new("health-plugin-disabled", "{ $plugin } is disabled")
                            .with("plugin", name()),
                        Message::new("health-plugin-disabled-detail", "Log: { $log }")
                            .with("log", log_of(p)),
                        Some(open_folder(
                            p,
                            Message::new("health-fix-plugin-folder", "Open its folder"),
                        )),
                    ),
                    Status::NotGranted {
                        capability,
                        needed: false,
                    } => (
                        Level::Ok,
                        Message::new("health-plugin-unused", "{ $plugin } is not used yet")
                            .with("plugin", name()),
                        Message::new(
                            "health-plugin-unused-detail",
                            "It asks for { $capability } when a source first uses it.",
                        )
                        .with("capability", capability.clone()),
                        None,
                    ),
                    Status::NotGranted {
                        capability,
                        needed: true,
                    } => (
                        Level::Warning,
                        Message::new(
                            "health-plugin-not-granted",
                            "{ $plugin } waits for a permission",
                        )
                        .with("plugin", name()),
                        Message::new(
                            "health-plugin-not-granted-detail",
                            "It needs { $capability } before it can start.",
                        )
                        .with("capability", capability.clone()),
                        Some(Fix {
                            command: GRANT.into(),
                            args: json!({ "plugin": p.id, "capability": capability }),
                            label: Message::new("health-fix-plugin-grant", "Allow { $capability }")
                                .with("capability", capability.clone()),
                        }),
                    ),
                    Status::Crashed(why) => (
                        Level::Problem,
                        Message::new("health-plugin-crashed", "{ $plugin } does not start")
                            .with("plugin", name()),
                        Message::new("health-plugin-crashed-detail", "{ $why }. Log: { $log }")
                            .with("why", why.clone())
                            .with("log", log_of(p)),
                        Some(Fix {
                            command: "plugin.disable".into(),
                            args: json!({ "plugin": p.id }),
                            label: Message::new("health-fix-plugin-disable", "Disable it"),
                        }),
                    ),
                    Status::Unsupported(problems) => (
                        Level::Problem,
                        Message::new(
                            "health-plugin-unsupported",
                            "{ $plugin } does not suit this Helios",
                        )
                        .with("plugin", name()),
                        Message::new(
                            "health-plugin-unsupported-detail",
                            "{ $problems }. Update it.",
                        )
                        .with("problems", problems.join("; ")),
                        Some(open_folder(
                            p,
                            Message::new("health-fix-plugin-update", "Update it: open its folder"),
                        )),
                    ),
                };
                let mut f = Finding::new(PLUGINS, "", level, message, detail);
                f.id = format!("{PLUGINS}.{}", p.id);
                if let Some(fix) = fix {
                    f = f.fix(fix);
                }
                f
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(id: &str, status: Status) -> Plugin {
        Plugin {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            dir: PathBuf::from("/plugins").join(id),
            status,
            log: vec!["panicked at main.rs".into()],
        }
    }

    #[test]
    fn each_plugin_has_its_finding_and_fix() {
        let ctx = vec![
            plugin("org.example.fine", Status::Answers),
            plugin(
                "org.example.old",
                Status::Unsupported(vec!["interface `importer` version 9".into()]),
            ),
            plugin(
                "org.example.crash",
                Status::Crashed("exit status 101".into()),
            ),
        ];
        let found = Plugins.findings(&ctx);
        let ids: Vec<(&str, Level)> = found.iter().map(|f| (f.id.as_str(), f.level)).collect();
        assert_eq!(
            ids,
            [
                ("plugins.org.example.fine", Level::Ok),
                ("plugins.org.example.old", Level::Problem),
                ("plugins.org.example.crash", Level::Problem),
            ]
        );
        assert_eq!(found[1].fix.as_ref().unwrap().command, "library.openFolder");
        let crash = found[2].fix.as_ref().unwrap();
        assert_eq!(crash.command, "plugin.disable");
        assert_eq!(crash.args["plugin"], "org.example.crash");
    }

    #[test]
    fn a_permission_waited_for_warns_only_once_something_uses_the_plugin() {
        let waiting = |needed| {
            plugin(
                "org.example.photos",
                Status::NotGranted {
                    capability: "network".into(),
                    needed,
                },
            )
        };
        let ctx = vec![
            waiting(false),
            waiting(true),
            plugin("org.example.granted", Status::Answers),
        ];
        let found = Plugins.findings(&ctx);
        // Unused: fine, nothing to fix.
        assert_eq!(found[0].level, Level::Ok);
        assert_eq!(found[0].message.key, "health-plugin-unused");
        assert!(found[0].fix.is_none());
        // Used without its permission: a warning, granting it the fix.
        assert_eq!(found[1].level, Level::Warning);
        let grant = found[1].fix.as_ref().unwrap();
        assert_eq!(grant.command, GRANT);
        assert_eq!(grant.args["plugin"], "org.example.photos");
        assert_eq!(grant.args["capability"], "network");
        // Granted: it answers.
        assert_eq!(found[2].level, Level::Ok);
        let declared = Plugins.declaration();
        for f in &found {
            assert_eq!(declared.breach(PLUGINS, f), None);
        }
    }
}
