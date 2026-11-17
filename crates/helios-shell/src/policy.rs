//! Who may run a command, and how: the one place that decides a run's
//! origin and whether its dry run is honoured, before any handler sees it.
//! The node's endpoint, its batch node and the desktop's window commands
//! all ask here, so they agree.
//!
//! - **Origins** have one table ([`origin`]): `user`, `cli`, `control`,
//!   `midi`, `agent:NAME`, `script:NAME` and `remote:NAME`.
//! - **Who connected decides `user`** ([`over_connection`]): the `user`
//!   origin is the person at the desktop, so only the desktop's own
//!   in-process calls carry it. A call that came over a connection to the
//!   endpoint (the CLI, an agent's shell, a script, another machine) names
//!   who it is, but never the user.
//! - **A dry run never acts** ([`dry_run`]): a command that does not take
//!   `dry_run` would ignore it and run for real, so the dry run is refused
//!   before the command runs.

use serde_json::Value;

use crate::commands::Origin;

/// The origins a run may name, as a refusal lists them.
pub const ORIGINS: &str = "user, cli, control, midi, agent:NAME, script:NAME or remote:NAME";

/// The origin named `name`: `user`, `midi`, `agent:NAME`, `script:NAME`,
/// `remote:NAME`; `cli` and `control`, a remote of that name. A name is
/// a word: not empty, no spaces.
///
/// # Errors
/// Not one of these forms.
pub fn origin(name: &str) -> Result<Origin, String> {
    let named = |prefix: &str| {
        name.strip_prefix(prefix)
            .filter(|n| !n.is_empty() && !n.contains(char::is_whitespace))
            .map(String::from)
    };
    match name {
        "user" => Ok(Origin::User),
        "midi" => Ok(Origin::Midi),
        "cli" | "control" => Ok(Origin::Remote(name.into())),
        _ => named("agent:")
            .map(Origin::Agent)
            .or_else(|| named("script:").map(Origin::Script))
            .or_else(|| named("remote:").map(Origin::Remote))
            .ok_or_else(|| format!("`{name}` is not an origin: {ORIGINS}")),
    }
}

/// Whether a call that came over a connection to the endpoint may run as
/// `origin`: any origin but the user's.
///
/// # Errors
/// The `user` origin, which only the desktop's own calls carry.
pub fn over_connection(origin: &Origin) -> Result<(), String> {
    match origin {
        Origin::User => Err(
            "a connection to the endpoint cannot run as `user`, the person at the desktop: \
             name who it is (cli, control, agent:NAME, script:NAME or remote:NAME)"
                .into(),
        ),
        _ => Ok(()),
    }
}

/// Whether `args` ask for a dry run: `dry_run` (or `dryRun`) given and
/// not `false`.
pub fn asks_dry_run(args: &Value) -> bool {
    ["dry_run", "dryRun"]
        .iter()
        .any(|k| args.get(k).is_some_and(|v| *v != Value::Bool(false)))
}

/// Whether a command's declared arguments (free JSON: an example or a
/// schema, as egui-pop's commands declare them) name `dry_run` (or
/// `dryRun`).
pub fn declares_dry_run(declared: &Value) -> bool {
    ["dry_run", "dryRun"].iter().any(|k| {
        declared.get(k).is_some()
            || declared
                .get("properties")
                .is_some_and(|p| p.get(k).is_some())
    })
}

/// Whether command `id` may run with `args`: a dry run is refused unless
/// the command takes one (`takes_dry_run`) or only reads (`read_only`:
/// it changes nothing either way).
///
/// # Errors
/// A dry run of a command that would run for real.
pub fn dry_run(id: &str, takes_dry_run: bool, read_only: bool, args: &Value) -> Result<(), String> {
    if asks_dry_run(args) && !takes_dry_run && !read_only {
        return Err(format!(
            "`{id}` takes no `dry_run`: it would run for real, so the dry run is refused"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn origins_are_one_table() {
        for (name, is) in [
            ("user", Origin::User),
            ("midi", Origin::Midi),
            ("cli", Origin::Remote("cli".into())),
            ("control", Origin::Remote("control".into())),
            ("agent:mcp", Origin::Agent("mcp".into())),
            ("script:batch", Origin::Script("batch".into())),
            ("remote:phone", Origin::Remote("phone".into())),
        ] {
            assert_eq!(origin(name), Ok(is), "{name}");
        }
        for bad in ["", "agent:", "remote:", "script: x", "robot", "User"] {
            assert!(origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_connection_names_anyone_but_the_user() {
        assert!(over_connection(&Origin::User).is_err());
        assert!(over_connection(&Origin::Agent("mcp".into())).is_ok());
        assert!(over_connection(&Origin::Remote("cli".into())).is_ok());
    }

    #[test]
    fn a_dry_run_runs_only_where_it_is_taken() {
        let dry = json!({ "dry_run": true });
        assert!(dry_run("theme.select", false, false, &dry).is_err());
        assert!(dry_run("theme.select", false, false, &json!({ "dryRun": 1 })).is_err());
        assert!(dry_run("theme.select", false, false, &json!({ "dry_run": false })).is_ok());
        assert!(dry_run("theme.select", false, false, &json!({})).is_ok());
        assert!(dry_run("macro.open", true, false, &dry).is_ok());
        assert!(dry_run("about.read", false, true, &dry).is_ok());
        assert!(declares_dry_run(
            &json!({ "scope": "app", "dry_run": false })
        ));
        assert!(declares_dry_run(&json!({ "file": "", "dryRun": false })));
        assert!(declares_dry_run(
            &json!({ "type": "object", "properties": { "dry_run": { "type": "boolean" } } })
        ));
        assert!(!declares_dry_run(&json!({ "name": "" })));
        assert!(!declares_dry_run(&Value::Null));
    }
}
