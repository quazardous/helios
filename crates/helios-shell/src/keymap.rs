//! Keys bound to commands, resolved by the framework alone (see
//! `docs/shortcuts-research.md`).
//!
//! Bindings come in layers: the user's over the workspace's over the
//! framework's defaults over the suggestions components make. Within a
//! layer the most specific `when` wins, then the last declared. A binding
//! to `-command` removes that command's bindings from the layers below,
//! for good. Keys are logical: what the key produces, so top-row and keypad
//! digits are the same. A shortcut is one or more chords (`Ctrl+K Ctrl+S`).

use std::fmt;

use serde::Serialize;

use crate::when::{Context, When};

/// Where a binding comes from; later variants win.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Suggestion,
    Default,
    Workspace,
    User,
}

/// One chord: modifiers and a logical key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// Lowercase key name: `a`, `3`, `f1`, `space`, `tab`, `\\`.
    pub key: String,
}

impl Chord {
    pub fn parse(text: &str) -> Result<Chord, String> {
        let mut c = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: String::new(),
        };
        let parts: Vec<&str> = text.split('+').collect();
        let (mods, key) = parts.split_at(parts.len() - 1);
        for m in mods {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" | "option" => c.alt = true,
                "shift" => c.shift = true,
                "meta" | "super" | "cmd" => c.meta = true,
                other => return Err(format!("`{other}` is not a modifier in `{text}`")),
            }
        }
        let key = key[0].to_lowercase();
        if key.is_empty() {
            return Err(format!("no key in `{text}`"));
        }
        // Keypad digits are digits.
        c.key = key.strip_prefix("numpad").unwrap_or(&key).to_string();
        Ok(c)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.meta, "Meta"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        let mut k = self.key.clone();
        if let Some(first) = k.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        write!(f, "{k}")
    }
}

/// A key sequence: one or more chords.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Keys(pub Vec<Chord>);

impl Keys {
    pub fn parse(text: &str) -> Result<Keys, String> {
        let chords = text
            .split_whitespace()
            .map(Chord::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if chords.is_empty() {
            return Err("no keys".into());
        }
        Ok(Keys(chords))
    }
}

impl fmt::Display for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(Chord::to_string).collect();
        write!(f, "{}", parts.join(" "))
    }
}

/// A binding as declared.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Binding {
    pub keys: Keys,
    /// A command id, or `-id` to remove the command's bindings below.
    pub command: String,
    pub when: String,
    pub layer: Layer,
    /// Who declared it: a module, a plugin, `framework`, `user`.
    pub source: String,
}

struct Rule {
    binding: Binding,
    when: When,
    order: usize,
}

/// What a key sequence does in a context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Run this command.
    Run(String),
    /// The keys so far start a longer shortcut: wait for the next chord.
    Pending,
    Nothing,
}

#[derive(Default)]
pub struct Keymap {
    rules: Vec<Rule>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(
        &mut self,
        keys: &str,
        command: &str,
        when: &str,
        layer: Layer,
        source: &str,
    ) -> Result<(), String> {
        let binding = Binding {
            keys: Keys::parse(keys)?,
            command: command.into(),
            when: when.into(),
            layer,
            source: source.into(),
        };
        let when = When::parse(when).map_err(|e| format!("`{keys}` → `{command}`: {e}"))?;
        let order = self.rules.len();
        self.rules.push(Rule {
            binding,
            when,
            order,
        });
        Ok(())
    }

    /// Removals: commands taken away at a layer, from the layers below.
    fn removed(&self, command: &str, below: Layer) -> bool {
        self.rules.iter().any(|r| {
            r.binding.layer > below && r.binding.command.strip_prefix('-') == Some(command)
        })
    }

    /// The bindings in force, after removals.
    fn live(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(|r| {
            !r.binding.command.starts_with('-')
                && !self.removed(&r.binding.command, r.binding.layer)
        })
    }

    /// The winner among bindings of exactly these keys in a context.
    fn winner(&self, keys: &Keys, cx: &Context) -> Option<&Rule> {
        self.live()
            .filter(|r| &r.binding.keys == keys && r.when.eval(cx))
            .max_by_key(|r| (r.binding.layer, r.when.specificity(), r.order))
    }

    /// The bindings declared at a layer, in order (removals included).
    pub fn bindings(&self, layer: Layer) -> Vec<&Binding> {
        self.rules
            .iter()
            .filter(|r| r.binding.layer == layer)
            .map(|r| &r.binding)
            .collect()
    }

    /// Forgets every binding of a layer (before loading another profile's).
    pub fn clear(&mut self, layer: Layer) {
        self.rules.retain(|r| r.binding.layer != layer);
    }

    /// What pressing `keys` (the chords so far) does here.
    pub fn resolve(&self, keys: &Keys, cx: &Context) -> Resolution {
        if let Some(r) = self.winner(keys, cx) {
            return Resolution::Run(r.binding.command.clone());
        }
        let longer = self.live().any(|r| {
            r.binding.keys.0.len() > keys.0.len()
                && r.binding.keys.0.starts_with(&keys.0)
                && r.when.eval(cx)
        });
        if longer {
            Resolution::Pending
        } else {
            Resolution::Nothing
        }
    }

    /// The keys bound to a command (what the palette and tooltips show).
    pub fn keys_for(&self, command: &str) -> Vec<Keys> {
        let mut keys: Vec<Keys> = self
            .live()
            .filter(|r| r.binding.command == command)
            .map(|r| r.binding.keys.clone())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Bindings sharing keys and overlapping contexts with another of the
    /// same or a higher layer that wins over them: what the shortcuts
    /// editor shows as conflicts, losing suggestions included.
    pub fn conflicts(&self) -> Vec<(Binding, Binding)> {
        let live: Vec<&Rule> = self.live().collect();
        let mut out = Vec::new();
        for (i, a) in live.iter().enumerate() {
            for b in &live[i + 1..] {
                if a.binding.keys == b.binding.keys && a.binding.command != b.binding.command {
                    let (lose, win) = if (a.binding.layer, a.when.specificity(), a.order)
                        < (b.binding.layer, b.when.specificity(), b.order)
                    {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    out.push((lose.binding.clone(), win.binding.clone()));
                }
            }
        }
        out
    }
}
