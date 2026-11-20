//! Helios's destructive commands: those an agent may run only once the
//! user says yes, as egui-pop's `CommandSpec::destructive` means it.
//!
//! The list is kept here, in one place, so a review sees it whole. Each
//! command in [`DESTRUCTIVE`] is declared `.destructive()` to egui-pop
//! (`plugins::declared`): an agent's call is held behind a confirmation
//! naming the agent and its arguments, and runs once the user accepts
//! (egui-pop's `pop.agent.confirm`); the user's own runs are never asked.
//!
//! A command is destructive when it deletes or forgets data (photos,
//! albums, keywords, templates, versions, nodes), sends files to the
//! trash or overwrites them, signs an account out, or resets more than
//! one value at once. One value set back to its default stays an edit:
//! the undo history has it.
//!
//! A command destructive only with some arguments is in
//! [`DESTRUCTIVE_IF`] instead: only those calls wait for the user's yes
//! (egui-pop's `CommandSpec::destructive_if`), the others run at once.
//!
//! Formatting or ejecting a card has no command yet (the storage page
//! acts on volumes itself); a command for it goes in this list.

/// Every command of Helios an agent runs only with the user's yes.
pub const DESTRUCTIVE: &[&str] = &[
    // Deletes or forgets: photos, versions, albums, keywords, templates.
    "library.collection.delete",
    "library.collection.remove",
    "library.deleteRejected",
    "library.keyword.merge",
    "library.keyword.remove",
    "library.metadata.template.delete",
    "library.quick.clear",
    "library.remove",
    "library.removeVersion",
    "library.trash",
    // Deletes nodes, wires or sub-workspaces of the network.
    "mask.member.delete",
    "network.disconnect",
    "network.remove",
    "subworkspace.delete",
    // Overwrites: marks, settings.
    "library.marks.paste",
    "profile.import.apply",
    // Signs out or disconnects an account, its tokens forgotten.
    "import.disconnect",
    "import.signOut",
    // Resets more than one value.
    "edit.resetAll",
    "library.resetRotation",
    "node.reset",
    "workspace.reset",
];

/// Commands destructive only when an argument has a value: the
/// command, the argument and the value. An agent's call with it waits for
/// the user's yes; its other calls run at once.
pub const DESTRUCTIVE_IF: &[(&str, &str, &str)] = &[
    // Writes new files, but replaces those of the same name with
    // `clash: overwrite`.
    ("library.export", "clash", "overwrite"),
];

/// Whether an agent must ask the user before any run of `command`
/// ([`DESTRUCTIVE`]).
pub fn is_destructive(command: &str) -> bool {
    DESTRUCTIVE.contains(&command)
}

/// The arguments and values that make a call of `command` destructive
/// ([`DESTRUCTIVE_IF`]).
pub fn destructive_if(command: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    DESTRUCTIVE_IF
        .iter()
        .filter(move |(c, _, _)| *c == command)
        .map(|(_, arg, value)| (*arg, *value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commands of the English catalogue (generated from Helios's
    /// declarations), each with its title.
    fn catalogue() -> Vec<(String, String)> {
        let ftl = include_str!("../../../i18n/en/commands.ftl");
        let mut out: Vec<(String, String)> = Vec::new();
        for line in ftl.lines() {
            if let Some(id) = line.strip_prefix("command-") {
                let id = id.trim_end_matches(" =").trim().replace('-', ".");
                out.push((id, String::new()));
            } else if let Some(title) = line.trim().strip_prefix(".title = ")
                && let Some((_, t)) = out.last_mut()
            {
                *t = title.to_string();
            }
        }
        out
    }

    /// Words of a command's id or title that say it may destroy.
    const WORDS: &[&str] = &[
        "delete",
        "remove",
        "reset",
        "trash",
        "sign an",
        "signout",
        "disconnect",
        "overwrite",
        "empty",
        "forget",
        "format",
        "eject",
        "clear",
        "purge",
    ];

    /// Commands whose id or title says one of [`WORDS`] and that are not
    /// destructive, and why.
    const NOT_DESTRUCTIVE: &[(&str, &str)] = &[
        ("compare.clear", "empties the comparison view, no data"),
        ("compare.remove", "takes photos out of the comparison view"),
        (
            "controls.reset",
            "one control to its default, one undo step",
        ),
        ("param.reset", "one value to its default, one undo step"),
        (
            "keymap.package.remove",
            "takes keys away that keymap.package adds back",
        ),
        (
            "library.collection.rules.edit",
            "edits a draft in the rule editor, each edit undoable",
        ),
        (
            "library.files.pair",
            "merges a raw and its JPEG into one photo; Ctrl+Z takes it back",
        ),
        (
            "library.metadata.set",
            "sets or removes metadata; Ctrl+Z takes it back",
        ),
        ("library.show", "clears the filter, a view"),
        (
            "library.quick.saveAs",
            "saves an album; emptying the quick collection after is asked by `clear`",
        ),
        (
            "macro.fold",
            "folds a macro back; a change made while open is asked first",
        ),
        (
            "mask.point.remove",
            "removes a path point, an undoable edit",
        ),
        (
            "network.connect",
            "a wire it `replaces` is an undoable network edit",
        ),
        (
            "param.set",
            "`reset` sets one value to its default, an undoable edit",
        ),
        ("theme.exportMerged", "writes a new file the user names"),
        ("vault.flag.none", "takes a mark off, undoable"),
        ("vault.flag.reject", "a mark, undoable"),
        ("vault.reject", "a mark, undoable"),
        ("window.maximize", "the window's size"),
        (
            "workshop.answer",
            "experimental; accept, override or reject a proposal, each an undo step",
        ),
        ("workshop.design", "experimental; one undo step"),
        (
            "workshop.propose",
            "experimental; ghost values the user accepts",
        ),
        (
            "workshop.review",
            "experimental; revert is the user's alone",
        ),
        (
            "workshop.session",
            "experimental; keep or close is the user's",
        ),
    ];

    #[test]
    fn a_command_whose_words_say_it_destroys_is_declared_or_explained() {
        let catalogue = catalogue();
        assert!(catalogue.len() > 200, "the catalogue was read");
        let unmarked: Vec<_> = catalogue
            .iter()
            .filter(|(id, title)| {
                let text = format!("{id} {title}").to_lowercase();
                WORDS.iter().any(|w| text.contains(w))
            })
            .map(|(id, _)| id.as_str())
            .filter(|id| {
                !is_destructive(id)
                    && destructive_if(id).next().is_none()
                    && !NOT_DESTRUCTIVE.iter().any(|(n, _)| n == id)
            })
            .collect();
        assert!(
            unmarked.is_empty(),
            "these commands may destroy: add them to destructive::DESTRUCTIVE \
             (or DESTRUCTIVE_IF), or to NOT_DESTRUCTIVE with the reason: {unmarked:?}"
        );
    }

    #[test]
    fn the_list_names_each_command_once_and_every_name_exists() {
        let mut ids = DESTRUCTIVE.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DESTRUCTIVE.len(), "a command listed twice");
        let catalogue = catalogue();
        let conditional = DESTRUCTIVE_IF.iter().map(|(c, _, _)| c);
        for id in DESTRUCTIVE
            .iter()
            .chain(conditional.clone())
            .chain(NOT_DESTRUCTIVE.iter().map(|(n, _)| n))
        {
            assert!(catalogue.iter().any(|(c, _)| c == id), "{id} is gone");
        }
        for id in DESTRUCTIVE.iter().chain(conditional.clone()) {
            assert!(
                !NOT_DESTRUCTIVE.iter().any(|(n, _)| n == id),
                "{id} is in both lists"
            );
        }
        for id in conditional {
            assert!(
                !is_destructive(id),
                "{id} is destructive whole and per call"
            );
        }
    }
}
