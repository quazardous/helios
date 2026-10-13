//! Messages for people, by key: what a layer tells a user (a GPU that
//! fell back, an export that failed, an import that left items out) is a
//! [`Message`], a translation key with named arguments and its English.
//!
//! The layers below the interface have no language: they build the
//! message, and whoever shows it (the desktop, an agent's client) looks
//! the key up in its catalogues with the arguments, or shows the English
//! when it has none. The English travels with the message, so a log, a
//! JSON line or a terminal still reads.
//!
//! ```
//! use helios_core::Message;
//!
//! let m = Message::new("engine-gpu-fell-back", "{ $module } ran on the CPU: { $reason }")
//!     .with("module", "exposure")
//!     .with("reason", "out of memory");
//! assert_eq!(m.key, "engine-gpu-fell-back");
//! assert_eq!(m.to_string(), "exposure ran on the CPU: out of memory");
//! ```
//!
//! The English is a Fluent pattern whose only placeables are arguments
//! (`{ $name }`): `cargo xtask i18n extract` finds every
//! `Message::new("key", "English")` written with literals and writes it
//! to the English catalogue (`i18n/en/messages.ftl`), where translations
//! may use all of Fluent (plurals, number options). Keys are written
//! `<area>-<what>` (`engine-…`, `export-…`, `xmp-…`, `gui-…`).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A message for people: a key, its arguments and its English.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// The catalogue key; empty for text that has none (an error from a
    /// library, a path), shown as is.
    pub key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, Arg>,
    /// The English, arguments placed.
    pub english: String,
    /// The English pattern, kept to place arguments added later.
    #[serde(skip)]
    pattern: String,
}

/// An argument of a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Arg {
    Number(f64),
    Text(String),
    /// A message of its own, translated before it is placed (a reason).
    Message(Box<Message>),
    /// Items placed one after the other with a separator.
    List(List),
}

/// Items of a list argument and what goes between them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct List {
    pub items: Vec<Arg>,
    pub separator: String,
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.args == other.args && self.english == other.english
    }
}

impl Message {
    /// A message from its key and its English pattern.
    pub fn new(key: &str, english: &str) -> Self {
        Message {
            key: key.into(),
            args: BTreeMap::new(),
            english: english.into(),
            pattern: english.into(),
        }
    }

    /// Text without a key, shown as it is in every language.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Message {
            key: String::new(),
            args: BTreeMap::new(),
            english: text.clone(),
            pattern: text,
        }
    }

    /// Whether the message has a key to translate.
    pub fn is_keyed(&self) -> bool {
        !self.key.is_empty()
    }

    /// Adds an argument and places it in the English.
    #[must_use]
    pub fn with(mut self, name: &str, value: impl Into<Arg>) -> Self {
        self.args.insert(name.into(), value.into());
        if self.is_keyed() {
            self.english = place(&self.pattern, &self.args);
        }
        self
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.english)
    }
}

impl From<String> for Message {
    fn from(text: String) -> Self {
        Message::plain(text)
    }
}

impl From<&str> for Message {
    fn from(text: &str) -> Self {
        Message::plain(text)
    }
}

impl From<Message> for String {
    fn from(m: Message) -> Self {
        m.english
    }
}

impl Arg {
    /// The argument in English.
    pub fn english(&self) -> String {
        match self {
            Arg::Number(n) => number(*n),
            Arg::Text(t) => t.clone(),
            Arg::Message(m) => m.english.clone(),
            Arg::List(l) => l
                .items
                .iter()
                .map(Arg::english)
                .collect::<Vec<_>>()
                .join(&l.separator),
        }
    }

    /// A list of arguments joined by `separator`.
    pub fn list(items: impl IntoIterator<Item = impl Into<Arg>>, separator: &str) -> Arg {
        Arg::List(List {
            items: items.into_iter().map(Into::into).collect(),
            separator: separator.into(),
        })
    }
}

/// A number as English writes it in a message: integers without a
/// fraction.
fn number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// The pattern with each `{ $name }` replaced by its argument's English;
/// a placeable without an argument stays as it is.
fn place(pattern: &str, args: &BTreeMap<String, Arg>) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = after[..close].trim();
        match inner.strip_prefix('$').and_then(|n| args.get(n.trim())) {
            Some(arg) => out.push_str(&arg.english()),
            None => out.push_str(&rest[open..=open + 1 + close]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

macro_rules! numbers {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg {
            fn from(n: $t) -> Self {
                Arg::Number(f64::from(n))
            }
        }
    )*};
}
numbers!(f64, f32, i32, u32);

// Counts and sizes: exact below 2^53, far beyond what a message counts.
macro_rules! wide {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg {
            fn from(n: $t) -> Self {
                Arg::Number(n as f64)
            }
        }
    )*};
}
wide!(i64, u64, usize);

impl From<String> for Arg {
    fn from(t: String) -> Self {
        Arg::Text(t)
    }
}

impl From<&str> for Arg {
    fn from(t: &str) -> Self {
        Arg::Text(t.into())
    }
}

impl From<&String> for Arg {
    fn from(t: &String) -> Self {
        Arg::Text(t.clone())
    }
}

impl From<Message> for Arg {
    fn from(m: Message) -> Self {
        Arg::Message(Box::new(m))
    }
}

impl From<&std::path::Path> for Arg {
    fn from(p: &std::path::Path) -> Self {
        Arg::Text(p.display().to_string())
    }
}

impl From<&std::path::PathBuf> for Arg {
    fn from(p: &std::path::PathBuf) -> Self {
        Arg::Text(p.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keys through a variable: the extractor reads literal keys only, and
    // these are not the catalogue's.
    fn msg(key: &str, english: &str) -> Message {
        Message::new(key, english)
    }

    #[test]
    fn arguments_are_placed_in_the_english() {
        let m = msg("k", "{ $n } of {$total} items: { $why }")
            .with("n", 2_usize)
            .with("total", 3.0)
            .with("why", msg("r", "no { $what }").with("what", "module"));
        assert_eq!(m.english, "2 of 3 items: no module");
        let unknown = msg("k", "{ $missing } stays");
        assert_eq!(unknown.english, "{ $missing } stays");
        let l = msg("k", "{ $names }").with("names", Arg::list(["a", "b"], ", "));
        assert_eq!(l.to_string(), "a, b");
        assert_eq!(msg("k", "x").with("v", 1.5).args["v"].english(), "1.5");
    }

    #[test]
    fn plain_text_has_no_key_and_keeps_its_braces() {
        let m = Message::plain("{ $x }").with("x", "y");
        assert!(!m.is_keyed());
        assert_eq!(m.english, "{ $x }");
    }

    #[test]
    fn a_message_goes_through_json() {
        let m = msg("export-failed", "cannot write { $path }")
            .with("path", "/tmp/a.jpg")
            .with("why", msg("why", "full"))
            .with("n", 2_u32);
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["key"], "export-failed");
        assert_eq!(json["english"], "cannot write /tmp/a.jpg");
        assert_eq!(json["args"]["n"], 2.0);
        assert_eq!(json["args"]["why"]["key"], "why");
        let back: Message = serde_json::from_value(json).unwrap();
        assert_eq!(back, m);
    }
}
