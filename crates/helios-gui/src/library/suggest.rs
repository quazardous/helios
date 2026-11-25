//! The search bar's suggestions and chips, as data
//! (`docs/design/library-search.md`, sections 2 and 5; slice G4).
//!
//! Helios draws no widget: this module says *what* the bar offers and
//! *how* its keyboard moves, and the search field of egui-pop draws it.
//! - [`key_suggestions`]: the declared keys ([`SearchKeys`]) the word
//!   being typed starts (`cam` offers `camera:`), and [`complete_key`]
//!   writing the one taken into the text.
//! - [`Suggestions`]: the keyboard of section 5 over a suggestion list
//!   (Up, Down, Enter, Tab, and the Esc sequence: close the list, clear
//!   the text, back to the grid).
//! - [`chips`]: the parsed search text (`helios_vault::rules::parse_text`)
//!   in words, one chip per term (`camera is Z 8`).

use helios_core::Message;
use helios_vault::rules::{Match, Rule};

use super::search_keys::{SearchKey, SearchKeys};
use crate::rule_editor_ui::{field_message, op_message};

/// Where the word being typed starts in `text`, after a `-`, `+` or `(`
/// before it; `None` when the text ends with a space or the word already
/// holds a key (a `:` or an operator).
fn typed_key(text: &str) -> Option<usize> {
    let start = text.rfind(char::is_whitespace).map_or(0, |i| {
        i + text[i..].chars().next().map_or(1, char::len_utf8)
    });
    let lead = text[start..]
        .char_indices()
        .find(|(_, c)| !matches!(c, '-' | '+' | '('))
        .map(|(i, _)| start + i)?;
    let word = &text[lead..];
    let is_key = |c: char| c.is_alphanumeric() || c == '_';
    (!word.is_empty() && word.chars().all(is_key)).then_some(lead)
}

/// The keys the word being typed at the end of `text` starts, in the
/// keys' order (case ignored). Empty when nothing is being typed, or when
/// the word is already a whole key followed by its colon.
pub fn key_suggestions<'k>(keys: &'k SearchKeys, text: &str) -> Vec<&'k SearchKey> {
    let Some(at) = typed_key(text) else {
        return Vec::new();
    };
    let word = text[at..].to_lowercase();
    keys.iter()
        .filter(|k| k.key.to_lowercase().starts_with(&word))
        .collect()
}

/// `text` with the word being typed replaced by `key:`, its `-`, `+` or
/// `(` kept: the value comes next. A text where no key is being typed
/// gets ` key:` at its end.
pub fn complete_key(text: &str, key: &str) -> String {
    match typed_key(text) {
        Some(at) => format!("{}{key}:", &text[..at]),
        None if text.is_empty() || text.ends_with(char::is_whitespace) => {
            format!("{text}{key}:")
        }
        None => format!("{text} {key}:"),
    }
}

/// A key the suggestion keyboard takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Enter,
    Tab,
    Escape,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The highlight moved, or the list opened.
    Moved,
    /// Take the suggestion at this index.
    Take(usize),
    /// The list closed.
    Closed,
    /// Clear the text typed (the chips stay: Esc never clears them).
    Clear,
    /// Leave the field, back to the grid.
    Leave,
    /// Not the list's key: the field keeps it.
    Ignored,
}

/// The suggestion list's state: open or not, the highlighted row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestions {
    open: bool,
    highlight: Option<usize>,
}

impl Suggestions {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn highlight(&self) -> Option<usize> {
        self.highlight.filter(|_| self.open)
    }

    /// The text changed: the list opens on what it now offers (`count`
    /// rows), nothing highlighted.
    pub fn typed(&mut self, count: usize) {
        self.open = count > 0;
        self.highlight = None;
    }

    /// Section 5's keys over a list of `count` rows; `text_empty` says
    /// whether the field holds any text (the Esc sequence).
    pub fn key(&mut self, key: Key, count: usize, text_empty: bool) -> Outcome {
        let open = self.open && count > 0;
        match key {
            Key::Down if count > 0 => {
                self.open = true;
                self.highlight = Some(match self.highlight.filter(|_| open) {
                    Some(i) => (i + 1).min(count - 1),
                    None => 0,
                });
                Outcome::Moved
            }
            Key::Up if open => {
                self.highlight = self.highlight.and_then(|i| i.checked_sub(1));
                Outcome::Moved
            }
            Key::Enter if open => match self.highlight {
                Some(i) => self.take(i.min(count - 1)),
                None => Outcome::Ignored,
            },
            // Tab completes the key: the highlighted one, else the first.
            Key::Tab if open => self.take(self.highlight.unwrap_or(0).min(count - 1)),
            Key::Escape if open => {
                *self = Suggestions::default();
                Outcome::Closed
            }
            Key::Escape if !text_empty => Outcome::Clear,
            Key::Escape => Outcome::Leave,
            _ => Outcome::Ignored,
        }
    }

    fn take(&mut self, i: usize) -> Outcome {
        *self = Suggestions::default();
        Outcome::Take(i)
    }
}

/// A suggestion row's words: the key's label and what it writes
/// (`Camera — camera:`).
pub fn key_row(key: &SearchKey, t: &dyn Fn(Message) -> String) -> String {
    t(
        Message::new("gui-library-suggest-key", "{ $label } — { $key }:")
            .with("label", t(key.label.clone()))
            .with("key", key.key.clone()),
    )
}

/// The search text's rule tree in words, one chip per term of its top
/// `all` group (`camera is Z 8`, `rating is at least 3`,
/// `label is red or label is blue`, `not flag is reject`).
pub fn chips(rule: &Rule, t: &dyn Fn(Message) -> String) -> Vec<String> {
    match rule {
        Rule::Group {
            mode: Match::All,
            rules,
        } => rules.iter().map(|r| words(r, t)).collect(),
        other => vec![words(other, t)],
    }
}

/// One rule in words.
fn words(rule: &Rule, t: &dyn Fn(Message) -> String) -> String {
    match rule {
        Rule::Condition(c) => {
            let value = match &c.value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Array(b) if b.len() == 2 => t(Message::new(
                    "gui-library-chip-range",
                    "{ $low } and { $high }",
                )
                .with("low", plain(&b[0]))
                .with("high", plain(&b[1]))),
                // A relative period and the like: as the text writes it.
                _ => {
                    let text = helios_vault::rules::to_text(rule);
                    text.split_once(':')
                        .map_or(text.clone(), |(_, v)| v.to_string())
                }
            };
            t(
                Message::new("gui-library-chip", "{ $field } { $op } { $value }")
                    .with("field", t(field_message(c.field.name)))
                    .with("op", t(op_message(c.op)))
                    .with("value", value),
            )
        }
        Rule::Group { mode, rules } => {
            let parts: Vec<String> = rules.iter().map(|r| words(r, t)).collect();
            match mode {
                Match::All => parts.join(", "),
                Match::Any => {
                    let or = t(Message::new("gui-library-chip-or", "or"));
                    parts.join(&format!(" {or} "))
                }
                Match::None => {
                    let or = t(Message::new("gui-library-chip-or", "or"));
                    t(Message::new("gui-library-chip-not", "not { $terms }")
                        .with("terms", parts.join(&format!(" {or} "))))
                }
            }
        }
    }
}

/// A JSON value as plain text (a number without quotes).
fn plain(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn english(m: Message) -> String {
        helios_i18n::message(&helios_i18n::I18n::new("en"), &m)
    }

    #[test]
    fn typing_a_key_s_start_offers_it_and_completes_it() {
        let keys = SearchKeys::builtin();
        let names = |text: &str| -> Vec<String> {
            key_suggestions(&keys, text)
                .iter()
                .map(|k| k.key.clone())
                .collect()
        };
        assert_eq!(names("cam"), ["camera"]);
        assert_eq!(names("beach CAM"), ["camera"]);
        assert_eq!(names("-fl"), ["flag"]);
        assert_eq!(names("c"), ["camera", "cull", "collection"]);
        // Nothing typed, a key already written, a word no key starts.
        assert!(names("").is_empty());
        assert!(names("cam ").is_empty());
        assert!(names("camera:").is_empty());
        assert!(names("rating>=3").is_empty());
        assert!(names("zzz").is_empty());

        assert_eq!(complete_key("cam", "camera"), "camera:");
        assert_eq!(complete_key("beach -fl", "flag"), "beach -flag:");
        assert_eq!(complete_key("(la", "label"), "(label:");
        assert_eq!(complete_key("", "tag"), "tag:");
        assert_eq!(complete_key("beach ", "tag"), "beach tag:");
        assert_eq!(complete_key("rating>=3", "tag"), "rating>=3 tag:");
    }

    #[test]
    fn down_then_enter_takes_the_highlighted_suggestion() {
        let mut s = Suggestions::default();
        s.typed(2);
        assert!(s.is_open());
        assert_eq!(s.highlight(), None);
        // Enter with nothing highlighted is the field's.
        assert_eq!(s.key(Key::Enter, 2, false), Outcome::Ignored);
        assert_eq!(s.key(Key::Down, 2, false), Outcome::Moved);
        assert_eq!(s.key(Key::Down, 2, false), Outcome::Moved);
        assert_eq!(s.key(Key::Down, 2, false), Outcome::Moved);
        assert_eq!(s.highlight(), Some(1), "Down stops at the last row");
        assert_eq!(s.key(Key::Up, 2, false), Outcome::Moved);
        assert_eq!(s.key(Key::Enter, 2, false), Outcome::Take(0));
        assert!(!s.is_open());
        // Tab completes the first key when none is highlighted.
        s.typed(3);
        assert_eq!(s.key(Key::Tab, 3, false), Outcome::Take(0));
        // Down opens a closed list.
        assert_eq!(s.key(Key::Down, 3, false), Outcome::Moved);
        assert_eq!(s.highlight(), Some(0));
    }

    #[test]
    fn escape_closes_then_clears_then_leaves() {
        let mut s = Suggestions::default();
        s.typed(1);
        assert_eq!(s.key(Key::Escape, 1, false), Outcome::Closed);
        assert_eq!(s.key(Key::Escape, 1, false), Outcome::Clear);
        assert_eq!(s.key(Key::Escape, 0, true), Outcome::Leave);
        // Closed, the arrows and Enter are the field's.
        assert_eq!(s.key(Key::Up, 0, true), Outcome::Ignored);
        assert_eq!(s.key(Key::Enter, 0, true), Outcome::Ignored);
    }

    #[test]
    fn the_parsed_text_reads_as_chips_in_words() {
        let rule = helios_vault::rules::parse_text(
            "camera:\"Z 8\" rating:3 label:red label:blue -flag:reject",
        )
        .unwrap();
        let read = chips(&rule, &english);
        assert!(read.contains(&"camera is Z 8".to_string()), "{read:?}");
        assert!(
            read.contains(&"rating is at least 3".to_string()),
            "{read:?}"
        );
        assert!(
            read.iter()
                .any(|c| c.contains("red") && c.contains(" or ") && c.contains("blue")),
            "{read:?}"
        );
        assert!(
            read.iter()
                .any(|c| c.contains("flag") && c.contains("reject")),
            "{read:?}"
        );
        let range = helios_vault::rules::parse_text("iso:100..800").unwrap();
        assert_eq!(chips(&range, &english), ["ISO is between 100 and 800"]);
    }

    #[test]
    fn a_key_row_says_its_label_and_term() {
        let keys = SearchKeys::builtin();
        assert_eq!(
            key_row(keys.get("camera").unwrap(), &english),
            "Camera — camera:"
        );
    }
}
