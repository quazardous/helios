//! The search's Advanced rows (`docs/design/library-search.md`, section 3,
//! slice G5): the search's rule tree drawn inline under the field, each
//! row a rule (field, operator, value), with the rule editor's boxes
//! ([`crate::rule_editor_ui::rows`]).
//!
//! The field and the rows are two views of one text, read and written
//! with the vault's grammar (`helios_vault::rules::parse_text` and
//! `to_text`): an edit in a row rewrites the search's text (its chips and
//! the words typed), and a text changed in the field (typed, a chip
//! removed, a command) draws the rows again. A row not valid yet (a value
//! being typed) keeps the text as it was, its fault said under it.

use helios_vault::rules::{Match, Rule, parse_text, to_text};
use serde_json::Value;

use crate::rule_editor::{Edit, RuleEditor};

/// The rows' state: the tree as edited, and the search's whole text it
/// mirrors.
#[derive(Clone, Debug)]
pub struct Advanced {
    pub(super) editor: RuleEditor,
    /// The search's whole text the tree was last read from or written to.
    text: String,
}

impl Advanced {
    /// The rows of the search whose whole text is `whole`.
    pub fn of(whole: &str) -> Advanced {
        let mut rows = Advanced {
            editor: RuleEditor::new("", "", &Rule::group(Match::All, Vec::new())),
            text: String::new(),
        };
        rows.read(whole);
        rows
    }

    /// Follows the search's whole text when it changed outside the rows:
    /// the rows are its tree again. A text the grammar refuses (a `(` not
    /// closed yet) leaves the rows as they are.
    pub fn follow(&mut self, whole: &str) {
        if whole != self.text {
            self.read(whole);
        }
    }

    fn read(&mut self, whole: &str) {
        if let Ok(rule) = parse_text(whole) {
            self.editor = RuleEditor::new("", "", &top(rule));
        }
        self.text = whole.to_string();
    }

    /// Makes the edit a row asked (`library.collection.rules.edit`'s
    /// arguments); answers the search's new text when the tree is valid,
    /// `None` while a row is not (or the edit changed nothing to write).
    pub fn edit(&mut self, args: &Value) -> Option<String> {
        self.editor.edit(Edit::parse(args).ok()?).ok()?;
        let rule = self.editor.tree.rule().ok()?;
        let text = to_text(&rule);
        (text != self.text).then_some(text)
    }

    /// The search's whole text once the rows' text is written in it (its
    /// chips first, then the words): the rows already show it.
    pub fn written(&mut self, whole: String) {
        self.text = whole;
    }
}

/// The tree with a group on top, so a row can be added beside a lone rule.
fn top(rule: Rule) -> Rule {
    match rule {
        group @ Rule::Group { .. } => group,
        other => Rule::group(Match::All, vec![other]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_row_s_value_rewrites_the_text_and_the_text_redraws_the_rows() {
        let mut rows = Advanced::of("camera:R5 sunset");
        let crate::rule_editor::Node::Group { rules, .. } = &rows.editor.tree else {
            panic!("a group on top");
        };
        assert_eq!(rules.len(), 2, "one row per term");

        let text = rows
            .edit(&json!({ "edit": "value", "at": [0], "value": "Z 8" }))
            .unwrap();
        assert_eq!(text, "camera:\"Z 8\" sunset");
        rows.written(text.clone());
        // Written back unchanged: the rows stay as edited.
        rows.follow(&text);
        assert!(rows.editor.can_undo(), "not read again");

        // An empty value is not a rule yet: the text stays.
        assert_eq!(
            rows.edit(&json!({ "edit": "value", "at": [0], "value": "" })),
            None
        );

        // A row added: a rule more.
        let mut rows = Advanced::of("camera:R5");
        let text = rows
            .edit(&json!({ "edit": "add_condition", "at": [] }))
            .unwrap();
        assert!(text.starts_with("camera:R5 rating"), "{text}");

        // Typed in the field: the rows follow.
        rows.follow("type:nef");
        let crate::rule_editor::Node::Group { rules, .. } = &rows.editor.tree else {
            panic!("a group on top");
        };
        assert_eq!(rules.len(), 1);
    }
}
