//! The plugin checks: each installed plugin starts, answers its
//! handshake, and speaks interface versions this Helios supports. They
//! read the [`Plugin`]s the desktop probed once per run (off the
//! interface thread); one finding per plugin, `plugins.<id>`.

use std::path::PathBuf;
use std::sync::Arc;

use helios_core::Message;
use serde_json::json;

use crate::{Check, Context, Declaration, Finding, Fix, Level};

/// The owner of the plugins' check.
pub const OWNER: &str = "plugins";

/// The plugin host's checks: each plugin starts and answers.
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![Arc::new(Plugins)]
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

impl Check for Plugins {
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

    fn run(&self, ctx: &Context) -> Vec<Finding> {
        ctx.plugins
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
        let ctx = Context {
            plugins: vec![
                plugin("org.example.fine", Status::Answers),
                plugin(
                    "org.example.old",
                    Status::Unsupported(vec!["interface `importer` version 9".into()]),
                ),
                plugin(
                    "org.example.crash",
                    Status::Crashed("exit status 101".into()),
                ),
            ],
            ..Context::default()
        };
        let found = Plugins.run(&ctx);
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
        let ctx = Context {
            plugins: vec![
                waiting(false),
                waiting(true),
                plugin("org.example.granted", Status::Answers),
            ],
            ..Context::default()
        };
        let found = Plugins.run(&ctx);
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
