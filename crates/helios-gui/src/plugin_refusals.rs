//! The plugin tools this desktop refused, and why: kept as the plugins'
//! rack declarations are read ([`crate::plugin_controls::read`]), and
//! answered by the read-only [`COMMAND`] (`helios.plugins.refusals`), a
//! window command the `helios` CLI reaches through the library's command
//! endpoint: `helios doctor plugins` lists them.
//!
//! A refusal is a tool left out (an unknown or `custom` handle kind, a
//! handle writing anything but a declared number, no handle left) or a
//! tool on the host's parameters held back while the plugin does not
//! hold `edit.controls`. Each plugin's list is replaced each time its
//! declarations are read; a refusal already known keeps the time it was
//! first seen.

use std::sync::{Mutex, PoisonError};

use helios_shell::commands::CommandSpec;
use serde::Serialize;
use serde_json::{Value, json};

/// The read-only command answering the refusals.
pub const COMMAND: &str = "helios.plugins.refusals";

/// A plugin tool refused, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub plugin: String,
    pub tool: String,
    pub reason: String,
    /// When it was first seen (RFC 3339, UTC).
    pub at: String,
}

/// The refusals known, plugin by plugin.
#[derive(Debug, Default)]
pub struct Refusals(Mutex<Vec<Refusal>>);

impl Refusals {
    /// The process's refusals.
    pub fn global() -> &'static Refusals {
        static GLOBAL: Refusals = Refusals(Mutex::new(Vec::new()));
        &GLOBAL
    }

    /// `plugin`'s refusals from now on, each `(tool, reason)`, seen `at`
    /// (those already known keep their time).
    pub fn record(&self, plugin: &str, refused: &[(String, String)], at: &str) {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let before: Vec<Refusal> = all.iter().filter(|r| r.plugin == plugin).cloned().collect();
        all.retain(|r| r.plugin != plugin);
        for (tool, reason) in refused {
            let at = before
                .iter()
                .find(|r| &r.tool == tool && &r.reason == reason)
                .map_or_else(|| at.to_owned(), |r| r.at.clone());
            all.push(Refusal {
                plugin: plugin.to_owned(),
                tool: tool.clone(),
                reason: reason.clone(),
                at,
            });
        }
    }

    /// Every refusal known, by plugin then tool.
    pub fn list(&self) -> Vec<Refusal> {
        let mut all = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        all.sort_by(|a, b| (&a.plugin, &a.tool).cmp(&(&b.plugin, &b.tool)));
        all
    }

    /// [`COMMAND`]'s answer: `{"refusals": [{plugin, tool, reason, at}]}`.
    pub fn answer(&self) -> Value {
        json!({ "refusals": self.list() })
    }
}

/// Now, as a refusal's time.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Registers [`COMMAND`] on the shell.
///
/// # Errors
/// The command cannot be registered.
pub fn register(shell: &mut crate::shell::Shell) -> Result<(), String> {
    shell.register(
        CommandSpec::new(
            COMMAND,
            "The plugin tools this desktop refused (plugin, tool, reason, when first seen): what `helios doctor plugins` lists",
            "Help",
        )
        .read_only(),
        |_, _| Ok(Refusals::global().answer()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(tool: &str, reason: &str) -> (String, String) {
        (tool.into(), reason.into())
    }

    #[test]
    fn a_recorded_refusal_is_answered_and_keeps_its_first_time() {
        let r = Refusals::default();
        r.record(
            "org.example.spot",
            &[refused("ring", "kind `custom` is not drawn by Helios")],
            "2026-11-25T10:00:00Z",
        );
        r.record(
            "org.example.other",
            &[refused("dial", "no handle left, left out")],
            "2026-11-25T10:00:01Z",
        );
        let answer = r.answer();
        assert_eq!(
            answer["refusals"][1],
            json!({ "plugin": "org.example.spot", "tool": "ring",
                    "reason": "kind `custom` is not drawn by Helios",
                    "at": "2026-11-25T10:00:00Z" })
        );
        assert_eq!(answer["refusals"][0]["plugin"], "org.example.other");

        // Read again: the same refusal keeps its time, a new one has its own,
        // one gone is dropped.
        r.record(
            "org.example.spot",
            &[
                refused("ring", "kind `custom` is not drawn by Helios"),
                refused("arm", "needs `edit.controls`"),
            ],
            "2026-11-25T11:00:00Z",
        );
        let spot: Vec<_> = r
            .list()
            .into_iter()
            .filter(|x| x.plugin == "org.example.spot")
            .collect();
        assert_eq!(spot.len(), 2);
        assert_eq!(spot[0].tool, "arm");
        assert_eq!(spot[0].at, "2026-11-25T11:00:00Z");
        assert_eq!(spot[1].at, "2026-11-25T10:00:00Z");
        r.record("org.example.spot", &[], "2026-11-25T12:00:00Z");
        assert_eq!(r.list().len(), 1);
    }
}
