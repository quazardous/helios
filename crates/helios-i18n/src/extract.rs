//! The English catalogue, generated from what Helios declares: its
//! commands, its panels, its modules' manifests, and what the desktop
//! declares to egui-pop beyond them (categories, menus, settings). Nobody
//! keeps a list of the ids built at run time: the declarations are that
//! list. The files are written by egui-pop's extractor.

use std::collections::BTreeMap;

use egui_pop_i18n::extract::{self, English, Found};

use helios_core::{Manifest, ParamHint, ParamKind};
use helios_shell::commands::CommandSpec;
use helios_shell::panels::PanelSpec;
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
}

impl Declarations {
    /// From the declarations themselves: command specs, panel specs, and
    /// each module's manifest with its parameter hints.
    pub fn from_specs<'a>(
        commands: impl IntoIterator<Item = &'a CommandSpec>,
        panels: impl IntoIterator<Item = &'a PanelSpec>,
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
            panels: panels
                .into_iter()
                .map(|p| Panel {
                    id: p.id.clone(),
                    title: p.title.clone(),
                })
                .collect(),
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
        }
    }

    /// Every message key with its English text.
    pub fn english(&self) -> BTreeMap<MessageId, String> {
        self.areas()
            .into_values()
            .flat_map(std::iter::IntoIterator::into_iter)
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
    /// `panels.ftl`, `modules.ftl`, `interface.ftl`), messages sorted by
    /// id, so the same declarations always give the same bytes. Refused
    /// when two stable ids give the same message key with two texts;
    /// empty texts are left out (Fluent has no empty value).
    pub fn english_files(&self) -> Result<BTreeMap<&'static str, String>, String> {
        let mut areas = self.areas();
        let interface = areas.entry("interface.ftl").or_default();
        for (key, text) in &self.interface {
            let (id, attr) = egui_pop_i18n::catalog::split_key(key);
            interface.push((
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
        // Across files too: one key, one text.
        extract::messages(&all)?;
        Ok(files)
    }
}

const HEADER: &str = "\
# Generated from Helios's declarations by `cargo xtask i18n extract`:
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
        Declarations::from_specs(&commands, &[], [(&m, hints)])
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
        let d = Declarations::from_specs(&commands, &[], []);
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
}
