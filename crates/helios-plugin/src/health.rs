//! The problem checks a plugin declares (`[[health]]` in its manifest),
//! as health checks ([`helios_health::Check`]) run through the plugin's
//! process.
//!
//! [`register`] adds each plugin's checks to a health
//! [`Registry`](helios_health::Registry), owned by the plugin (its id),
//! each as `<plugin id>.<check id>`; one the registry refuses is left out
//! and its reasons returned (a manifest with an unsound declaration is
//! refused before, with the plugin).
//!
//! A run calls `health.check` (the `health` interface, version 1) with
//! `{check, accounts?, config}`: `check` the id the manifest gives,
//! `accounts` (a plugin with `[[connect]]`) its connector's accounts as
//! the host gives them (`id`, `label`, `state`, `auth`: an access token,
//! or null when there is none to give). It answers `{findings}`, each
//! `{about?, level, message, detail?, fix?}`, the messages as
//! [`helios_core::Message`] (`key`, `english`, `args`). A plugin that is
//! disabled or waits for a capability is not started (its checks say
//! nothing: the plugins' own check says why); one that does not start,
//! crashes, does not answer within the time limit or answers what is not
//! findings gives that check's "failed" problem ([`helios_health::failed`]).

use std::sync::Arc;
use std::time::Duration;

use helios_core::Message;
use helios_health::{Check, Context, Declaration, Finding, Fix, Level, Registry};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::Broker;
use crate::manifest::HealthDecl;

/// The method a plugin's check is run by.
pub const METHOD: &str = "health.check";

/// How long a plugin's check may take, its start included.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// A connector's accounts, as a check is handed them: each `{id, label,
/// state, auth}`, by the connector's id (the last part of its plugin's
/// id: `google-photos`).
pub type AccountsFn = Arc<dyn Fn(&str) -> Vec<Json> + Send + Sync>;

/// One check a plugin declares, run through its process.
pub struct PluginCheck {
    broker: Arc<Broker>,
    plugin: String,
    /// Its id in the plugin's manifest.
    local: String,
    /// `<plugin id>.<local>`.
    id: String,
    title: String,
    declaration: Declaration,
    accounts: Option<AccountsFn>,
    timeout: Duration,
}

impl PluginCheck {
    /// The check `decl` of the plugin `plugin`; `accounts` is handed to a
    /// plugin with `[[connect]]`.
    pub fn new(
        broker: Arc<Broker>,
        plugin: &str,
        decl: &HealthDecl,
        accounts: Option<AccountsFn>,
    ) -> Self {
        Self {
            broker,
            plugin: plugin.to_owned(),
            local: decl.id.clone(),
            id: format!("{plugin}.{}", decl.id),
            title: decl.title.clone(),
            declaration: decl.declaration(),
            accounts,
            timeout: TIMEOUT,
        }
    }

    /// Waits at most `timeout` for the plugin's answer.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The plugin it runs through.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    fn params(&self) -> Json {
        let mut params = json!({ "check": self.local });
        let connects = self
            .broker
            .manifest(&self.plugin)
            .is_some_and(|m| !m.connect.is_empty());
        if connects && let Some(accounts) = &self.accounts {
            let connector = self.plugin.rsplit('.').next().unwrap_or(&self.plugin);
            params["accounts"] = Json::Array(accounts(connector));
        }
        params
    }

    fn failed(&self, why: &str) -> Vec<Finding> {
        log::warn!("health: check `{}`: {why}", self.id);
        vec![helios_health::failed(&self.id, self.title())]
    }
}

/// A finding as a plugin answers it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answered {
    #[serde(default)]
    about: String,
    level: Level,
    message: Message,
    #[serde(default)]
    detail: Option<Message>,
    #[serde(default)]
    fix: Option<Fix>,
}

#[derive(Deserialize)]
struct Answer {
    findings: Vec<Answered>,
}

impl Check for PluginCheck {
    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> Message {
        Message::plain(self.title.clone())
    }

    fn declaration(&self) -> Declaration {
        self.declaration.clone()
    }

    fn run(&self, _: &Context) -> Vec<Finding> {
        let answer = match self
            .broker
            .health_check(&self.plugin, self.params(), self.timeout)
        {
            Ok(None) => return Vec::new(),
            Ok(Some(answer)) => answer,
            Err(why) => return self.failed(&why),
        };
        match serde_json::from_value::<Answer>(answer) {
            Ok(a) => a
                .findings
                .into_iter()
                .map(|f| {
                    let detail = f.detail.unwrap_or_else(|| Message::plain(""));
                    let found = Finding::new(&self.id, &f.about, f.level, f.message, detail);
                    match f.fix {
                        Some(fix) => found.fix(fix),
                        None => found,
                    }
                })
                .collect(),
            Err(e) => self.failed(&format!("its answer is not findings: {e}")),
        }
    }
}

/// Each plugin's checks, owned by the plugin: its id, and the check.
pub fn checks(broker: &Arc<Broker>, accounts: Option<&AccountsFn>) -> Vec<(String, PluginCheck)> {
    broker
        .ids()
        .filter_map(|id| Some((id, broker.manifest(id)?)))
        .flat_map(|(id, m)| {
            m.health.iter().map(move |decl| {
                (
                    id.to_owned(),
                    PluginCheck::new(broker.clone(), id, decl, accounts.cloned()),
                )
            })
        })
        .collect()
}

/// Adds every plugin's checks to `registry`, each owned by its plugin;
/// gives the reasons of those it refused (logged too).
pub fn register(
    registry: &mut Registry,
    broker: &Arc<Broker>,
    accounts: Option<&AccountsFn>,
) -> Vec<String> {
    let mut refused = Vec::new();
    for (owner, check) in checks(broker, accounts) {
        if let Err(problems) = registry.add(&owner, Arc::new(check)) {
            for p in problems {
                log::warn!("health: plugin `{owner}`: {p}");
                refused.push(format!("plugin `{owner}`: {p}"));
            }
        }
    }
    refused
}
