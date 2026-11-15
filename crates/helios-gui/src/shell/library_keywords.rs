//! The Library's keyword commands (`docs/design/library.md`, L12): the
//! Keywords section's scope, tagging and untagging, and a branch renamed
//! or merged on every photo (asked in a form or a chooser when the new
//! path is not given), each a step Undo takes back.

use egui_pop::api::dialogs::{self, Choose, ChooseOption};
use egui_pop::model::ids::CommandId;
use helios_core::Message;
use serde_json::{Value, json};

use super::Shell;
use crate::library::{
    KEYWORD_ADD_COMMAND, KEYWORD_COMMAND, KEYWORD_LIST_COMMAND, KEYWORD_MERGE_COMMAND,
    KEYWORD_REMOVE_COMMAND, KEYWORD_RENAME_COMMAND,
};

/// The commands, with their titles (category Library).
pub(super) const COMMANDS: &[(&str, &str)] = &[
    (
        KEYWORD_COMMAND,
        "Scope the library to a keyword branch (`path`, levels joined with `|`: the photos with that keyword or one below it; null: none), with the other filters. Without `path`, says the branch",
    ),
    (
        KEYWORD_ADD_COMMAND,
        "Tag photos with a keyword (`keyword`, levels joined with `|`; `images`: ids, else the selection), a mark as `library.tag` makes it",
    ),
    (
        KEYWORD_REMOVE_COMMAND,
        "Take a keyword off photos, with those below it (`keyword`; `images`: ids, else the selection), a mark as `library.tag` makes it",
    ),
    (
        KEYWORD_RENAME_COMMAND,
        "Rename a keyword branch on every photo, the keywords below it along (`from`, `to`: a keyword no photo has yet); without `to`, asks it; Ctrl+Z takes it back",
    ),
    (
        KEYWORD_MERGE_COMMAND,
        "Merge a keyword branch into another photos have, on every photo (`from`, `into`); without `into`, asks it; Ctrl+Z takes it back",
    ),
    (
        KEYWORD_LIST_COMMAND,
        "List the keyword branches (`path`, levels joined with `|`) with how many photos have a keyword of each (`count`)",
    ),
];

impl Shell {
    /// Runs one of [`COMMANDS`].
    pub(super) fn library_keywords_command(
        &mut self,
        id: &str,
        args: &Value,
    ) -> Result<Value, String> {
        let text = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
        };
        let images = || -> Result<Option<Vec<String>>, String> {
            match args.get("images") {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Array(list)) => Ok(Some(
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect(),
                )),
                Some(other) => Err(format!("`images`: {other} is not a list of ids")),
            }
        };
        match id {
            KEYWORD_COMMAND => {
                let mut lib = self.focus.library.borrow_mut();
                match args.get("path") {
                    None => {}
                    Some(Value::Null) => {
                        lib.set_keyword(None);
                        lib.record_filter();
                    }
                    Some(Value::String(path)) => {
                        let path = helios_vault::keywords::path(path)?;
                        lib.set_keyword(Some(&path));
                        lib.record_filter();
                    }
                    Some(other) => return Err(format!("`path`: {other} is not a keyword path")),
                }
                Ok(json!({ "path": lib.keyword() }))
            }
            KEYWORD_ADD_COMMAND | KEYWORD_REMOVE_COMMAND => {
                let keyword = text("keyword").ok_or("`keyword`: no keyword named")?;
                let images = images()?;
                let mut lib = self.focus.library.borrow_mut();
                if id == KEYWORD_ADD_COMMAND {
                    Ok(json!({ "tagged": lib.add_keyword(&keyword, images)? }))
                } else {
                    Ok(json!({ "untagged": lib.remove_keyword(&keyword, images)? }))
                }
            }
            KEYWORD_RENAME_COMMAND => {
                let from = text("from").ok_or("`from`: no keyword named")?;
                let Some(to) = text("to") else {
                    self.keyword_rename_form(&from)?;
                    return Ok(Value::Null);
                };
                let photos = self
                    .focus
                    .library
                    .borrow_mut()
                    .ask_rename_keyword(&from, &to)?;
                Ok(json!({ "photos": photos }))
            }
            KEYWORD_MERGE_COMMAND => {
                let from = text("from").ok_or("`from`: no keyword named")?;
                let Some(into) = text("into") else {
                    self.keyword_merge_chooser(&from)?;
                    return Ok(Value::Null);
                };
                let photos = self
                    .focus
                    .library
                    .borrow_mut()
                    .ask_merge_keyword(&from, &into)?;
                Ok(json!({ "photos": photos }))
            }
            KEYWORD_LIST_COMMAND => {
                let mut lib = self.focus.library.borrow_mut();
                let list: Vec<Value> = lib
                    .keyword_counts()
                    .iter()
                    .map(|(path, count)| json!({ "path": path, "count": count }))
                    .collect();
                Ok(json!({ "keywords": list, "shown": lib.keyword() }))
            }
            other => Err(format!("no keyword command `{other}`")),
        }
    }

    /// The form a branch's new path is asked in, starting from its path.
    fn keyword_rename_form(&mut self, from: &str) -> Result<(), String> {
        let from = helios_vault::keywords::path(from)?;
        if !self
            .focus
            .library
            .borrow_mut()
            .keyword_counts()
            .contains_key(&from)
        {
            return Err(format!("no photo has the keyword “{from}”"));
        }
        let command = CommandId::new(KEYWORD_RENAME_COMMAND).map_err(|e| e.to_string())?;
        let title = self.text(
            Message::new("gui-library-keyword-rename-title", "Rename “{ $name }”")
                .with("name", from.as_str()),
        );
        let verb = self.text(Message::new("gui-library-keyword-rename-verb", "Rename"));
        let label = self.text(Message::new("gui-library-keyword-path", "keyword"));
        let mut form = dialogs::Form::new(&title, &verb, command).field_at(
            "to",
            crate::dialogs::descriptor(&helios_values::Descriptor::text(&label, &from)),
            crate::dialogs::value(&helios_core::Value::Text(from.clone())),
        );
        form.args = json!({ "from": from });
        self.open_dialog(form);
        Ok(())
    }

    /// "Merge into…": the other branches `from` may go into (not itself
    /// nor one below it); the choice runs the merge with its `into`.
    fn keyword_merge_chooser(&mut self, from: &str) -> Result<(), String> {
        let from = helios_vault::keywords::path(from)?;
        let targets: Vec<(String, u64)> = {
            let mut lib = self.focus.library.borrow_mut();
            let counts = lib.keyword_counts();
            if !counts.contains_key(&from) {
                return Err(format!("no photo has the keyword “{from}”"));
            }
            counts
                .iter()
                .filter(|(p, _)| !helios_vault::keywords::in_branch(p, &from))
                .map(|(p, n)| (p.clone(), *n))
                .collect()
        };
        if targets.is_empty() {
            self.notify(
                crate::dialogs::Level::Info,
                Message::new(
                    "gui-library-keyword-merge-none",
                    "No other keyword to merge it into",
                ),
            );
            return Ok(());
        }
        let options: Vec<ChooseOption> = targets
            .iter()
            .map(|(path, count)| ChooseOption::new(path, json!(path)).hint(&count.to_string()))
            .collect();
        let title = self.text(
            Message::new("gui-library-keyword-merge-title", "Merge “{ $name }” into")
                .with("name", from.as_str()),
        );
        let command = CommandId::new(KEYWORD_MERGE_COMMAND).map_err(|e| e.to_string())?;
        self.open_dialog(
            Choose::with_options(&title, options, command)
                .args(json!({ "from": from }))
                .value_key("into"),
        );
        Ok(())
    }
}
