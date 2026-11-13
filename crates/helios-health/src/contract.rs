//! The health contract: what a check declares beside its id and title,
//! so the [`Registry`](crate::Registry) can refuse an unsound one and the
//! runner can hold each run to it. A module or a plugin owns
//! its checks and declares, for each:
//!
//! - what it watches, in English, for people and tools reading the
//!   declarations;
//! - the [`Level`]s its findings may have;
//! - the fix commands its findings may offer.
//!
//! [`Declaration::problems`] lists what is wrong with a declaration (a
//! check with any is refused when added, with its reasons), and
//! [`Declaration::breach`] what is wrong with a finding against it: a
//! finding outside its check's declaration turns the run of that check
//! into a problem of its own, like a check that panics.

use serde::{Deserialize, Serialize};

use crate::{Finding, Level};

/// What a check declares: what it watches, the levels it may raise and
/// the fix commands it may offer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    /// What it watches, in English (one sentence).
    pub watches: String,
    /// The levels its findings may have.
    pub levels: Vec<Level>,
    /// The commands its findings' fixes may run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<String>,
}

impl Declaration {
    /// A check watching `watches` (in English).
    pub fn new(watches: &str) -> Self {
        Self {
            watches: watches.to_owned(),
            ..Self::default()
        }
    }

    /// The levels its findings may have.
    pub fn levels(mut self, levels: &[Level]) -> Self {
        self.levels = levels.to_vec();
        self
    }

    /// The commands its findings' fixes may run.
    pub fn fixes(mut self, commands: &[&str]) -> Self {
        self.fixes = commands.iter().map(|c| (*c).to_owned()).collect();
        self
    }

    /// What is wrong with the declaration of the check `id`; empty: it
    /// is sound.
    pub fn problems(&self, id: &str) -> Vec<String> {
        let mut p = Vec::new();
        if !sound_name(id) {
            p.push("a check id of lowercase letters, digits, `.`, `-` and `_`".to_owned());
        }
        if self.watches.trim().is_empty() {
            p.push("`watches` says what it watches".to_owned());
        }
        if self.levels.is_empty() {
            p.push("`levels` names at least one level".to_owned());
        }
        for (i, level) in self.levels.iter().enumerate() {
            if self.levels[..i].contains(level) {
                p.push(format!("`levels` names {level:?} twice"));
            }
        }
        for (i, fix) in self.fixes.iter().enumerate() {
            if !sound_command(fix) {
                p.push(format!(
                    "fix `{fix}`: a command id such as `library.storage`"
                ));
            } else if self.fixes[..i].contains(fix) {
                p.push(format!("fix `{fix}` is named twice"));
            }
        }
        p.into_iter()
            .map(|e| format!("check `{id}`: {e}"))
            .collect()
    }

    /// What is wrong with `finding` against the declaration of the check
    /// `id`: it names another check, has a level not declared or offers a
    /// fix not declared. `None`: it fits.
    pub fn breach(&self, id: &str, finding: &Finding) -> Option<String> {
        if finding.check != id {
            return Some(format!(
                "check `{id}` gave a finding of check `{}`",
                finding.check
            ));
        }
        if !self.levels.contains(&finding.level) {
            return Some(format!(
                "check `{id}` gave a finding at {:?}, a level it does not declare",
                finding.level
            ));
        }
        if let Some(fix) = &finding.fix
            && !self.fixes.contains(&fix.command)
        {
            return Some(format!(
                "check `{id}` offered the fix `{}`, which it does not declare",
                fix.command
            ));
        }
        None
    }
}

/// A check's or an owner's id: lowercase letters, digits, `.`, `-` and
/// `_`, starting with a letter.
pub(crate) fn sound_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// A command id (`library.openFolder`): letters, digits and `.`,
/// starting with a lowercase letter.
fn sound_command(command: &str) -> bool {
    command.starts_with(|c: char| c.is_ascii_lowercase())
        && command
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.')
}

#[cfg(test)]
mod tests {
    use helios_core::Message;
    use serde_json::json;

    use super::*;
    use crate::Fix;

    fn declared() -> Declaration {
        Declaration::new("The disks Helios writes to")
            .levels(&[Level::Ok, Level::Warning])
            .fixes(&["library.storage"])
    }

    #[test]
    fn a_sound_declaration_has_no_problems_and_reads_from_json() {
        assert_eq!(
            declared().problems("storage.free-space"),
            Vec::<String>::new()
        );
        let read: Declaration = serde_json::from_value(json!({
            "watches": "The disks Helios writes to",
            "levels": ["ok", "warning"],
            "fixes": ["library.storage"],
        }))
        .unwrap();
        assert_eq!(read, declared());
    }

    #[test]
    fn an_unsound_declaration_is_refused_with_its_reasons() {
        let bad = Declaration {
            watches: " ".into(),
            levels: vec![Level::Ok, Level::Ok],
            fixes: vec!["open folder".into(), "a.b".into(), "a.b".into()],
        };
        assert_eq!(
            bad.problems("Free Space"),
            vec![
                "check `Free Space`: a check id of lowercase letters, digits, `.`, `-` and `_`",
                "check `Free Space`: `watches` says what it watches",
                "check `Free Space`: `levels` names Ok twice",
                "check `Free Space`: fix `open folder`: a command id such as `library.storage`",
                "check `Free Space`: fix `a.b` is named twice",
            ]
        );
        assert_eq!(
            Declaration::new("x").problems("x"),
            vec!["check `x`: `levels` names at least one level"]
        );
    }

    #[test]
    fn a_finding_outside_its_declaration_is_a_breach() {
        let d = declared();
        let finding = |check: &str, level| {
            Finding::new(check, "", level, Message::plain("m"), Message::plain("d"))
        };
        let fix = |command: &str| Fix {
            command: command.into(),
            args: json!({}),
            label: Message::plain("Fix"),
        };
        assert_eq!(d.breach("a", &finding("a", Level::Warning)), None);
        assert_eq!(
            d.breach(
                "a",
                &finding("a", Level::Warning).fix(fix("library.storage"))
            ),
            None
        );
        assert_eq!(
            d.breach("a", &finding("a", Level::Problem)).as_deref(),
            Some("check `a` gave a finding at Problem, a level it does not declare")
        );
        assert_eq!(
            d.breach("a", &finding("a", Level::Ok).fix(fix("plugin.disable")))
                .as_deref(),
            Some("check `a` offered the fix `plugin.disable`, which it does not declare")
        );
        assert_eq!(
            d.breach("a", &finding("b", Level::Ok)).as_deref(),
            Some("check `a` gave a finding of check `b`")
        );
    }
}
