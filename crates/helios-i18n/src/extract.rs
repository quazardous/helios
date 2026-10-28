//! The English catalogue, generated from what Helios declares: its
//! commands, its panels, its modules' manifests, and what the desktop
//! declares to egui-pop beyond them (categories, menus, settings). Nobody
//! keeps a list of the ids built at run time: the declarations are that
//! list. The files are written by egui-pop's extractor.

use std::collections::BTreeMap;

use egui_pop_i18n::extract::{self, English, Found};

use helios_core::{Manifest, ParamHint, ParamKind};
use helios_shell::commands::CommandSpec;
use serde::{Deserialize, Serialize};

use crate::ids::{self, MessageId};

/// A command, as the catalogue needs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub id: String,
    pub title: String,
}

/// A panel, as the catalogue needs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    pub title: String,
}

/// A module's parameter, as the catalogue needs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    /// The short label shown (a hint's), else the name.
    pub label: String,
    pub description: String,
    /// A choice's values; empty for other kinds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// A module, as the catalogue needs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    pub params: Vec<Param>,
}

/// Everything that has English text to translate, in a form a tool can
/// pass around as JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Declarations {
    pub commands: Vec<Command>,
    pub panels: Vec<Panel>,
    pub modules: Vec<Module>,
    /// The rest of what the desktop declares to egui-pop, by key with its
    /// English (categories, submenus, settings, plugins), egui-pop's own
    /// keys left out: `interface.ftl`.
    #[serde(default)]
    pub interface: BTreeMap<String, String>,
    /// The messages the code builds (`Message::new("key", "English")`),
    /// by key with their English Fluent pattern: `messages.ftl`.
    #[serde(default)]
    pub messages: BTreeMap<String, String>,
    /// The tips' titles and texts (`tip-<id>.title`, `tip-<id>.text`),
    /// by key with their English Fluent pattern ([`tip_pattern`]):
    /// `tips.ftl`.
    #[serde(default)]
    pub tips: BTreeMap<String, String>,
}

/// The calls whose literal key and English the extractor reads from
/// Helios's sources: `Message::new("key", "English { $arg }")`.
pub const PATTERNS: &[extract::Pattern] = &[extract::Pattern {
    callee: "Message::new",
    key: 0,
    english: Some(1),
}];

/// The messages Helios's code builds: every `Message::new` with a literal
/// key and English in the `src` of each crate and module under `root`
/// (`crates/*/src`, `modules/*/src`), by key. Refused when one key is
/// given two English texts. Tests build their messages through a
/// variable key, so they never reach the catalogue.
pub fn code_messages(root: &std::path::Path) -> Result<BTreeMap<String, String>, String> {
    let mut found = Vec::new();
    for area in ["crates", "modules"] {
        let Ok(dirs) = std::fs::read_dir(root.join(area)) else {
            continue;
        };
        let mut dirs: Vec<_> = dirs.filter_map(|e| e.ok().map(|e| e.path())).collect();
        dirs.sort();
        for dir in dirs {
            let src = dir.join("src");
            if src.is_dir() {
                found.extend(
                    extract::scan_dir(&src, PATTERNS)
                        .map_err(|e| format!("{}: {e}", src.display()))?,
                );
            }
        }
    }
    Ok(extract::messages(&found)?
        .into_iter()
        .filter_map(|(k, e)| match e {
            English::Pattern(p) => Some((k, p)),
            _ => None,
        })
        .collect())
}

impl Declarations {
    /// From the declarations themselves: command specs, panels (an id and
    /// a title, as egui-pop's workbench declares them), and each module's
    /// manifest with its parameter hints.
    pub fn from_specs<'a>(
        commands: impl IntoIterator<Item = &'a CommandSpec>,
        panels: impl IntoIterator<Item = Panel>,
        modules: impl IntoIterator<Item = (&'a Manifest, Vec<ParamHint>)>,
    ) -> Self {
        Self {
            commands: commands
                .into_iter()
                .map(|c| Command {
                    id: c.id.clone(),
                    title: c.title.clone(),
                })
                .collect(),
            panels: panels.into_iter().collect(),
            modules: modules
                .into_iter()
                .map(|(m, hints)| Module {
                    name: m.name.clone(),
                    params: m
                        .params
                        .iter()
                        .map(|p| Param {
                            name: p.name.clone(),
                            label: hints
                                .iter()
                                .find(|h| h.name == p.name)
                                .and_then(|h| h.label.clone())
                                .unwrap_or_else(|| p.name.clone()),
                            description: p.description.clone(),
                            choices: match &p.kind {
                                ParamKind::Choice { options, .. } => options.clone(),
                                _ => Vec::new(),
                            },
                        })
                        .collect(),
                })
                .collect(),
            interface: BTreeMap::new(),
            messages: BTreeMap::new(),
            tips: BTreeMap::new(),
        }
    }

    /// Every message key derived from a declaration, with its English
    /// text (the code's messages, which are patterns, left out).
    pub fn english(&self) -> BTreeMap<MessageId, String> {
        self.areas()
            .into_values()
            .flat_map(std::iter::IntoIterator::into_iter)
            .collect()
    }

    /// Every key with its English, as the extractor finds them: the
    /// declarations' texts, the interface's, and the code's patterns.
    pub fn found(&self) -> Vec<Found> {
        let plain = self
            .english()
            .into_iter()
            .map(|(id, text)| (id.key(), English::Plain(text)))
            .chain(
                self.interface
                    .iter()
                    .map(|(k, t)| (k.clone(), English::Plain(t.clone()))),
            );
        let patterns = self
            .messages
            .iter()
            .chain(&self.tips)
            .map(|(k, p)| (k.clone(), English::Pattern(p.clone())));
        plain
            .chain(patterns)
            .filter(|(_, e)| match e {
                English::Plain(t) | English::Pattern(t) => !t.trim().is_empty(),
                English::None => false,
            })
            .map(|(key, english)| Found {
                key,
                english,
                origin: String::new(),
            })
            .collect()
    }

    /// The English units per area file, before ids are checked.
    fn areas(&self) -> BTreeMap<&'static str, Vec<(MessageId, String)>> {
        let mut areas: BTreeMap<&'static str, Vec<(MessageId, String)>> = BTreeMap::new();
        let commands = areas.entry("commands.ftl").or_default();
        for c in &self.commands {
            commands.push((ids::command_title(&c.id), c.title.clone()));
        }
        let panels = areas.entry("panels.ftl").or_default();
        for p in &self.panels {
            panels.push((ids::panel_title(&p.id), p.title.clone()));
        }
        let modules = areas.entry("modules.ftl").or_default();
        for m in &self.modules {
            for p in &m.params {
                modules.push((ids::param_label(&m.name, &p.name), p.label.clone()));
                modules.push((
                    ids::param_description(&m.name, &p.name),
                    p.description.clone(),
                ));
                for v in &p.choices {
                    modules.push((ids::param_choice(&m.name, &p.name, v), v.clone()));
                }
            }
        }
        areas
    }

    /// The English catalogue: one Fluent file per area (`commands.ftl`,
    /// `panels.ftl`, `modules.ftl`, `interface.ftl`, and `messages.ftl`
    /// for the messages the code builds), messages sorted by
    /// id, so the same declarations always give the same bytes. Refused
    /// when two stable ids give the same message key with two texts;
    /// empty texts are left out (Fluent has no empty value).
    pub fn english_files(&self) -> Result<BTreeMap<&'static str, String>, String> {
        let mut areas = self.areas();
        // A message lives in one file: an attribute of a message another
        // area holds (a panel command's description, which egui-pop
        // declares) goes beside it.
        let owner: BTreeMap<String, &'static str> = areas
            .iter()
            .flat_map(|(file, units)| {
                units.iter().map(|(id, _)| {
                    (
                        egui_pop_i18n::catalog::split_key(&id.key()).0.to_string(),
                        *file,
                    )
                })
            })
            .collect();
        for (key, text) in &self.interface {
            let (id, attr) = egui_pop_i18n::catalog::split_key(key);
            let file = owner.get(id).copied().unwrap_or("interface.ftl");
            areas.entry(file).or_default().push((
                match attr {
                    Some(a) => MessageId::attr(id, a),
                    None => MessageId::new(id),
                },
                text.clone(),
            ));
        }
        let mut all = Vec::new();
        let mut files = BTreeMap::new();
        for (file, units) in areas {
            let found: Vec<Found> = units
                .into_iter()
                .filter(|(_, text)| !text.trim().is_empty())
                .map(|(id, text)| Found {
                    key: id.key(),
                    english: English::Plain(text),
                    origin: file.to_string(),
                })
                .collect();
            all.extend(found.iter().cloned());
            let messages = extract::messages(&found)?;
            files.insert(file, extract::english(&messages, HEADER));
        }
        if !self.messages.is_empty() {
            let found: Vec<Found> = self
                .messages
                .iter()
                .map(|(key, pattern)| Found {
                    key: key.clone(),
                    english: English::Pattern(pattern.clone()),
                    origin: "messages.ftl".into(),
                })
                .collect();
            all.extend(found.iter().cloned());
            let messages = extract::messages(&found)?;
            files.insert("messages.ftl", extract::english(&messages, HEADER));
        }
        if !self.tips.is_empty() {
            let found: Vec<Found> = self
                .tips
                .iter()
                .map(|(key, pattern)| Found {
                    key: key.clone(),
                    english: English::Pattern(pattern.clone()),
                    origin: "tips.ftl".into(),
                })
                .collect();
            all.extend(found.iter().cloned());
            let messages = extract::messages(&found)?;
            files.insert("tips.ftl", extract::english(&messages, HEADER));
        }
        // Across files too: one key, one text.
        extract::messages(&all)?;
        Ok(files)
    }
}

/// A tip's text (`{key}`, `{key:<command>}`, as egui-pop's tips write
/// it) as a Fluent pattern: `{ $key }` and `{ $key-<command> }` (every
/// character of the command other than an ASCII letter, a digit or `-`
/// written `-`), Fluent's own placeholders, every other brace escaped.
pub fn tip_pattern(text: &str) -> String {
    // The placeholders are set aside while the rest is escaped.
    let mut names = Vec::new();
    let mut marked = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}').map(|i| i + open) else {
            break;
        };
        let inner = rest[open + 1..close].trim();
        let name = if inner == "key" {
            Some("key".to_string())
        } else {
            inner.strip_prefix("key:").map(|c| {
                let slug: String = c
                    .trim()
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() || ch == '-' {
                            ch
                        } else {
                            '-'
                        }
                    })
                    .collect();
                format!("key-{slug}")
            })
        };
        match name {
            Some(n) => {
                marked.push_str(&rest[..open]);
                marked.push('\u{E000}');
                marked.push_str(&names.len().to_string());
                marked.push('\u{E001}');
                names.push(n);
            }
            None => marked.push_str(&rest[..=close]),
        }
        rest = &rest[close + 1..];
    }
    marked.push_str(rest);
    let mut pattern = extract::escape(&marked);
    for (i, n) in names.iter().enumerate() {
        pattern = pattern.replace(&format!("\u{E000}{i}\u{E001}"), &format!("{{ ${n} }}"));
    }
    pattern
}

const HEADER: &str = "\
# Generated from Helios's declarations and code by `cargo xtask i18n extract`:
# do not edit. Translations live in i18n/<locale>/ under the same names.
";

#[cfg(test)]
mod tests {
    use super::*;
    use egui_pop_i18n::I18n;
    use helios_core::{ParamSpec, PortSpec};

    fn manifest() -> Manifest {
        Manifest {
            name: "demo".into(),
            version: 1,
            description: "a demo".into(),
            params: vec![
                ParamSpec::float("gain", "how much {louder}", 0.0, 1.0, 0.5),
                ParamSpec {
                    name: "mode".into(),
                    description: "how".into(),
                    kind: ParamKind::Choice {
                        options: vec!["fast".into(), "fine_grained".into()],
                        default: "fast".into(),
                    },
                },
            ],
            inputs: vec![PortSpec::image("input", 4)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![],
            needs: helios_core::Needs::default(),
            kind: helios_core::ModuleKind::Process,
            family: helios_core::Family::Tone,
            role: helios_core::Role::General,
        }
    }

    fn declarations() -> Declarations {
        let commands = [
            CommandSpec::new("view.palette", "Show all commands", "View"),
            CommandSpec::new("vault.rate.3", "Rate 3 stars", "Vault"),
        ];
        let m = manifest();
        let hints = vec![ParamHint::new("gain").label("Gain")];
        Declarations::from_specs(&commands, [], [(&m, hints)])
    }

    #[test]
    fn extraction_is_sorted_and_deterministic() {
        let d = declarations();
        let mut shuffled = d.clone();
        shuffled.commands.reverse();
        let a = d.english_files().unwrap();
        assert_eq!(a, shuffled.english_files().unwrap());
        assert_eq!(
            a["commands.ftl"].split_once("\n\n").unwrap().1,
            "command-vault-rate-3 =\n    .title = Rate 3 stars\n\n\
             command-view-palette =\n    .title = Show all commands\n"
        );
        let modules = &a["modules.ftl"];
        assert!(modules.contains("param-demo-gain =\n    .description = how much {\"{\"}louder{\"}\"}\n    .label = Gain\n"), "{modules}");
        assert!(modules.contains("\nparam-demo-mode-choice-fine-grained = fine_grained\n"));
        assert_eq!(a["panels.ftl"], HEADER);
    }

    #[test]
    fn the_generated_english_formats_back_to_the_declared_text() {
        let d = declarations();
        let i = I18n::new("en-US");
        for source in d.english_files().unwrap().values() {
            // Loaded as if it were a translation, to read it back.
            i.add("en-US", source).unwrap();
        }
        for (id, english) in d.english() {
            assert_eq!(i.get(&id.key(), &[]), Some(english), "{id}");
        }
    }

    #[test]
    fn two_ids_giving_one_key_are_refused() {
        let commands = [
            CommandSpec::new("a.b", "One", "X"),
            CommandSpec::new("a_b", "Two", "X"),
        ];
        let d = Declarations::from_specs(&commands, [], []);
        assert!(d.english_files().unwrap_err().contains("command-a-b.title"));
    }

    #[test]
    fn multi_line_text_survives() {
        let text = "first {line}\n[second]\n  .third";
        let mut d = Declarations::default();
        d.interface.insert("m".into(), text.into());
        let source = &d.english_files().unwrap()["interface.ftl"];
        let i = I18n::new("fr");
        i.add("fr", source).unwrap();
        assert_eq!(i.t("m", &[]), "first {line}\n[second]\n.third");
    }

    #[test]
    fn a_tip_s_keys_become_fluent_placeholders_in_tips_ftl() {
        assert_eq!(
            tip_pattern("{key} rates, {key:vault.flag.pick} picks, {other} stays."),
            "{ $key } rates, { $key-vault-flag-pick } picks, {\"{\"}other{\"}\"} stays."
        );
        let mut d = Declarations::default();
        d.tips.insert(
            "tip-demo-rate.text".into(),
            tip_pattern("{key:vault.rate.1} rates."),
        );
        d.tips.insert("tip-demo-rate.title".into(), "Rate".into());
        let source = &d.english_files().unwrap()["tips.ftl"];
        assert!(
            source.ends_with(
                "tip-demo-rate =\n    .text = { $key-vault-rate-1 } rates.\n    .title = Rate\n"
            ),
            "{source}"
        );
        let i = I18n::new("en");
        i.add("en", source).unwrap();
        let args = [("key-vault-rate-1", crate::FluentValue::from("`1`"))];
        assert_eq!(
            i.get("tip-demo-rate.text", &args),
            Some("`1` rates.".to_string())
        );
    }
}
