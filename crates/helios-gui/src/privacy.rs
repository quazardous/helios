//! Helios's private commands: those whose arguments or results carry
//! private data, as egui-pop's `CommandSpec::private` means it: places,
//! people, accounts, absolute paths and file names.
//!
//! The list is kept here, in one place, so a review sees it whole:
//!
//! - each command in [`PRIVATE`] is declared `.private()` to egui-pop
//!   (`plugins::declared`): an agent whose grant withholds
//!   private data (the Agents page's "Private data" switch, egui-pop's
//!   `Grant::private`) is refused it, the user never;
//! - its arguments are left out of the journal ([`redact`], applied by
//!   the host where egui-pop's events go on Helios's bus): the journal is
//!   what agents, the log's trace and a bug report read, and none of them
//!   needs the path or the account to follow what ran.
//!
//! A command's result is not redacted: egui-pop's MCP bridge reads a
//! queued command's result back from the journal's `command.done`, so an
//! agent allowed private data would lose it. That gap is egui-pop's to
//! close (a result answered on the call, the journal's entry redacted).

use serde_json::{Value as Json, json};

/// What a private command's arguments are replaced with in the journal.
pub const REDACTED: &str = "<private>";

/// Every command of Helios whose arguments or results carry private
/// data, by what they carry.
pub const PRIVATE: &[&str] = &[
    // Absolute paths and file names: folders, files, workspaces, exports.
    "calibrate.open",
    "device.download",
    "gui.record.start",
    "gui.snapshot",
    "health.findings",
    "init.apply",
    "init.chooseFolder",
    "init.libraryFolder",
    "library.chooseFolder",
    "library.copyFiles",
    "library.export",
    "library.folder",
    "library.importFiles",
    "library.importSource",
    "library.importWindow",
    "library.importWith",
    "library.openFolder",
    "library.pasteFiles",
    "library.scanFolder",
    "library.volumes",
    "metadata.read",
    "profile.export",
    "profile.import",
    "theme.exportMerged",
    "workbench.quickOpen.go",
    "workspace.importParts",
    "workspace.new",
    "workspace.open",
    "workspace.parts",
    // Accounts: their names, addresses, tokens and OAuth clients.
    "import.accounts",
    "import.connection",
    "import.connectionAnswer",
    "import.disconnect",
    "import.history.list",
    "import.oauthClients",
    "import.reconnect",
    "import.renameAccount",
    "import.setOAuthClient",
    "import.signOut",
    "import.sources.add",
    // People and places: metadata (creator, title, description),
    // keywords, album names and the filters naming them.
    "library.collection.create",
    "library.collection.list",
    "library.collection.rename",
    "library.filter.apply",
    "library.filter.pin",
    "library.filter.recent",
    "library.keyword",
    "library.keyword.add",
    "library.keyword.list",
    "library.keyword.merge",
    "library.keyword.remove",
    "library.keyword.rename",
    "library.marks.copy",
    "library.marks.paste",
    "library.metadata.get",
    "library.metadata.set",
    "library.metadata.template.apply",
    "library.metadata.template.list",
    "library.metadata.template.save",
    "library.quick.saveAs",
];

/// Whether `command` carries private data ([`PRIVATE`]).
pub fn is_private(command: &str) -> bool {
    PRIVATE.contains(&command)
}

/// An egui-pop event as the journal keeps it: a private command's
/// dispatch with its arguments replaced by [`REDACTED`]; any other event
/// as it is.
pub fn redact(name: &str, data: Json) -> Json {
    if name != "command.dispatched" {
        return data;
    }
    match data["command"].as_str() {
        Some(command) if is_private(command) => {
            json!({ "command": command, "args": REDACTED, "private": true })
        }
        _ => data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commands of the English catalogue (generated from Helios's
    /// declarations), each with the arguments its title names in
    /// backticks.
    fn catalogue() -> Vec<(String, Vec<String>)> {
        let ftl = include_str!("../../../i18n/en/commands.ftl");
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for line in ftl.lines() {
            if let Some(id) = line.strip_prefix("command-") {
                let id = id.trim_end_matches(" =").trim().replace('-', ".");
                out.push((id, Vec::new()));
            } else if let Some(title) = line.trim().strip_prefix(".title = ")
                && let Some((_, args)) = out.last_mut()
            {
                args.extend(title.split('`').skip(1).step_by(2).map(String::from));
            }
        }
        out
    }

    /// Arguments that name a file, a folder or an account.
    const PRIVATE_ARGS: &[&str] = &["path", "paths", "folder", "source", "account", "item"];

    /// Commands taking such an argument that is not private data, and why.
    const NOT_PRIVATE: &[(&str, &str)] = &[
        ("param.copyPath", "`path` names a value, not a file"),
        ("param.copyValue", "`path` names a value, not a file"),
        ("param.get", "`path` names a value, not a file"),
        ("param.paste", "`path` names a value, not a file"),
        ("param.reset", "`path` names a value, not a file"),
        ("param.set", "`path` names a value, not a file"),
    ];

    #[test]
    fn a_command_taking_a_path_folder_or_account_is_private() {
        let catalogue = catalogue();
        assert!(catalogue.len() > 200, "the catalogue was read");
        let unmarked: Vec<_> = catalogue
            .iter()
            .filter(|(_, args)| args.iter().any(|a| PRIVATE_ARGS.contains(&a.as_str())))
            .map(|(id, _)| id.as_str())
            .filter(|id| !is_private(id) && !NOT_PRIVATE.iter().any(|(n, _)| n == id))
            .collect();
        assert!(
            unmarked.is_empty(),
            "these commands take a path, a folder or an account: add them to \
             privacy::PRIVATE, or to NOT_PRIVATE with the reason: {unmarked:?}"
        );
    }

    #[test]
    fn the_list_names_each_command_once_and_the_exceptions_exist() {
        let mut ids = PRIVATE.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PRIVATE.len(), "a command listed twice");
        let catalogue = catalogue();
        for (id, _) in NOT_PRIVATE {
            assert!(catalogue.iter().any(|(c, _)| c == id), "{id} is gone");
        }
    }

    #[test]
    fn a_private_dispatch_loses_its_arguments_and_the_rest_is_kept() {
        let path = json!({ "path": "/home/someone/Pictures/2026" });
        let r = redact(
            "command.dispatched",
            json!({ "command": "library.folder", "args": path }),
        );
        assert_eq!(
            r,
            json!({ "command": "library.folder", "args": REDACTED, "private": true })
        );
        assert!(!r.to_string().contains("/home/"));
        let plain = json!({ "command": "vault.rate", "args": { "stars": 3 } });
        assert_eq!(redact("command.dispatched", plain.clone()), plain);
        let done = json!({ "command": "library.folder", "result": null });
        assert_eq!(redact("command.done", done.clone()), done);
    }
}
