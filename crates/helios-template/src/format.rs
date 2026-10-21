//! The files' shapes: `template.json` ([`Template`]) and a composite file
//! ([`CompositeFile`]). Every struct refuses fields it does not know, as
//! the schemas do (`templates/schema/`).

use std::collections::BTreeMap;

use egui_pop_model::panels::Area;
use helios_core::Params;
use serde::{Deserialize, Serialize};

/// The format this Helios writes and the newest it reads.
pub const FORMAT: u32 = 1;

/// The name a composite's own ports go by in its connections (`"from":
/// "@"`): never a node's name.
pub const PORTS: &str = "@";

/// `template.json`: what a template is, its network and its tools. Its
/// layout is a file of its own (`layout.json`, egui-pop's document).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// The format it is written in ([`FORMAT`]).
    pub format: u32,
    /// Stable, never translated: `helios.portrait-retouch`.
    pub id: String,
    /// The template's own version, bumped by its author.
    pub version: u32,
    pub meta: Meta,
    pub network: NetworkDesc,
    #[serde(default)]
    pub tools: Tools,
    /// What it sets for the library: an import source, how culling goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<LibraryDesc>,
    /// What is asked when a workspace is created from it ("ask on
    /// create"): the import source, a file a parameter names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    /// Named scopes over nodes of its network, nested or side by side
    /// (see [`crate::subworkspace`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subworkspaces: Vec<SubWorkspace>,
    /// The ids of sub-workspaces deleted: never given again in this
    /// workspace, so a stale reference (a context stack, an undo step)
    /// never lands on another one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired_subworkspaces: Vec<String>,
    /// Where the graph view draws each node, by name: the top-left
    /// corner in graph units. A view, not part of the network (the
    /// render never reads it); a node without one is placed by rank.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub positions: BTreeMap<String, [f32; 2]>,
}

/// A sub-workspace: a named scope over nodes of the workspace's own
/// network (one network, one engine graph). Entered, it shows and edits
/// only its nodes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubWorkspace {
    /// Stable, never reused in the workspace: a slug of its first name
    /// (`skin`, `skin-2`). What the context stack's payload names.
    pub id: String,
    /// Shown, in the user's words.
    pub name: String,
    /// Every node in it, those of the sub-workspaces nested in it
    /// included: a network node's name, or an opened composite's inner
    /// node (`<name>.<inner>`).
    pub nodes: Vec<String>,
    /// The sub-workspace it is nested in; at the top of the workspace
    /// without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// A value asked for when a workspace is created from a template: where
/// the answer goes in `template.json`, and what it may be. Without an
/// answer the template's own value stays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    /// Stable, written as a node's name: `import_source`.
    pub id: String,
    /// In English; translations under `<slug>-parameter-<id>`.
    pub name: String,
    /// Where the answer goes, as a JSON pointer into `template.json`
    /// (`/library/import/source`).
    pub at: String,
    pub kind: ParameterKind,
    /// The words a `choice` may be.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// What a parameter's answer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterKind {
    /// Free text.
    Text,
    /// A file or a directory on the machine the workspace is made on.
    Path,
    /// One of `choices`.
    Choice,
}

impl Template {
    /// Drops the positions of nodes no longer in the network (an opened
    /// composite's inner nodes go with it).
    pub fn follow_positions(&mut self) {
        let net = &self.network;
        let known = |name: &str| {
            let top = name.split('.').next().unwrap_or(name);
            net.sources.iter().any(|s| s == top) || net.nodes.iter().any(|n| n.name == top)
        };
        self.positions.retain(|name, _| known(name));
    }

    /// The template with answers to its parameters (by id) put where each
    /// one points; a parameter not answered keeps the template's value.
    /// An answer of the wrong kind, or to no parameter, is refused.
    pub fn answered(
        &self,
        answers: &BTreeMap<String, serde_json::Value>,
    ) -> Result<Template, String> {
        if answers.is_empty() {
            return Ok(self.clone());
        }
        let mut json = serde_json::to_value(self).map_err(|e| format!("template: {e}"))?;
        for (id, answer) in answers {
            let q = self
                .parameters
                .iter()
                .find(|q| q.id == *id)
                .ok_or_else(|| format!("the template asks no `{id}`"))?;
            let text = answer
                .as_str()
                .ok_or_else(|| format!("`{id}`: an answer is text"))?;
            if q.kind == ParameterKind::Choice && !q.choices.iter().any(|c| c == text) {
                return Err(format!(
                    "`{id}`: `{text}` is not one of {}",
                    q.choices.join(", ")
                ));
            }
            let slot = json
                .pointer_mut(&q.at)
                .ok_or_else(|| format!("`{id}`: the template has nothing at `{}`", q.at))?;
            *slot = serde_json::Value::String(text.to_string());
        }
        serde_json::from_value(json).map_err(|e| format!("template: {e}"))
    }
}

/// The library a template works with: where it imports from, and how
/// culling goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryDesc {
    /// The import source the workspace offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportDesc>,
    /// Culling moves to the next image after each mark.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_advance: bool,
}

/// An import source, written as `library.import`'s arguments are.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportDesc {
    /// What is imported: `card` (a mounted card, read from its `DCIM`) or
    /// `folder` (a folder and its subfolders).
    pub source: String,
    /// `reference` (files stay where they are) or `copy` (into the
    /// library folder).
    #[serde(default = "reference")]
    pub placement: String,
    /// A copy's new name, as `helios import --rename` takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    /// What to do with an exact duplicate: `skip`, `keep` or `ask`.
    #[serde(default = "skip")]
    pub duplicates: String,
    /// After each import, propose stacks (a `stacks.suggest` decision to
    /// review: nothing is applied until accepted).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suggest_stacks: bool,
}

fn reference() -> String {
    "reference".into()
}

fn skip() -> String {
    "skip".into()
}

/// The import sources a template may name.
pub const IMPORT_SOURCES: [&str; 2] = ["card", "folder"];
/// How imported files may be placed.
pub const PLACEMENTS: [&str; 2] = ["reference", "copy"];
/// What an import may do with a duplicate.
pub const DUPLICATES: [&str; 3] = ["skip", "keep", "ask"];

/// What people pick a template by.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meta {
    /// The English name; translations under `<slug>-name`.
    pub name: String,
    /// The scenario it is made for, in English; translations under
    /// `<slug>-scenario`.
    pub scenario: String,
    /// A picture of it, a file of the bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// A short generic icon name (`portrait`, `import`), shown beside its
    /// name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// The starting graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkDesc {
    /// The images the workspace is given, by node name; each one's output
    /// port is `image`.
    pub sources: Vec<String>,
    pub nodes: Vec<NodeDesc>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// The output the viewport shows.
    pub view: Endpoint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Preset>,
}

/// A node of a template's network: a module or a composite.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDesc {
    pub name: String,
    /// A module, by its manifest's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// A composite, by id and version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite: Option<CompositeRef>,
    /// Checked against the manifest, completed with defaults.
    #[serde(default, skip_serializing_if = "is_empty")]
    pub params: Params,
    /// A composite opened: its inner nodes in the graph as ordinary
    /// nodes (`<name>.<inner>`), which panels can edit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    /// Rendered as if absent, its parameters kept (see
    /// `helios_core::Graph::set_bypass`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypassed: bool,
    /// Overrides the composite's panel hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelHint>,
}

fn is_empty(p: &Params) -> bool {
    p.iter().next().is_none()
}

/// A composite pinned to a version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeRef {
    pub id: String,
    pub version: u32,
}

impl std::fmt::Display for CompositeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.id, self.version)
    }
}

/// An output port wired to an input port, by names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// A node, a source, or in a composite file [`PORTS`] for the
    /// composite's own inputs.
    pub from: String,
    pub port: String,
    pub to: String,
    pub input: String,
}

/// A node's output port.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub node: String,
    pub port: String,
}

/// Named parameter values, by node name: a module's parameters, a
/// composite's exposed ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub id: String,
    /// In English.
    pub name: String,
    pub values: BTreeMap<String, Params>,
}

/// The tools a workspace starts with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tools {
    /// Command ids, in the order shown.
    #[serde(default)]
    pub toolbox: Vec<String>,
    /// One of the toolbox's commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Bindings added to the workspace's keymap layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keymap: Vec<Binding>,
}

/// A key binding, keys written as a profile writes them (`Mod+Shift+P`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub keys: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
}

/// Which panel edits a composite and where it docks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelHint {
    pub panel: String,
    pub area: Area,
}

/// A composite saved as a file of its own: a sub-network, the
/// parameters it exposes and its panel hint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeFile {
    pub format: u32,
    pub id: String,
    pub version: u32,
    /// In English.
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    /// In order: a node reads the composite's inputs and nodes before it.
    pub nodes: Vec<InnerDesc>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    pub exits: Vec<Exit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed: Vec<Exposed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelHint>,
}

impl CompositeFile {
    /// What a template names it by.
    pub fn reference(&self) -> CompositeRef {
        CompositeRef {
            id: self.id.clone(),
            version: self.version,
        }
    }

    /// Its file name: `<id>@<version>.helios-composite.json`.
    pub fn file_name(&self) -> String {
        format!("{}{COMPOSITE_EXTENSION}", self.reference())
    }
}

/// A composite file's name ends so.
pub const COMPOSITE_EXTENSION: &str = ".helios-composite.json";

/// An image port of a composite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    pub name: String,
    pub channels: u32,
    /// What the channels mean (`rgba`, `y`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// An input that may stay unconnected.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl Port {
    pub fn spec(&self) -> helios_core::PortSpec {
        let mut spec = helios_core::PortSpec::image(&self.name, self.channels);
        spec.layout.clone_from(&self.layout);
        spec.optional = self.optional;
        spec
    }
}

/// An inner node of a composite: a module or a composite.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InnerDesc {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composite: Option<CompositeRef>,
    #[serde(default, skip_serializing_if = "is_empty")]
    pub params: Params,
}

/// An inner output made one of the composite's outputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exit {
    pub from: String,
    pub port: String,
    pub output: String,
}

/// An inner parameter made the composite's own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exposed {
    pub name: String,
    pub node: String,
    pub param: String,
}

/// Whether `id` is a template's or a composite's id: lowercase words of
/// letters and digits, joined by `.` or `-`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.split(['.', '-']).all(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Whether `name` is a node's name: a lowercase letter, then lowercase
/// letters, digits and `_`.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The message keys a template's text goes under: its id with every
/// character other than an ASCII letter or digit written `-`, as a
/// plugin's.
pub fn slug(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_names() {
        assert!(valid_id("helios.portrait-retouch"));
        assert!(valid_id("a1"));
        for bad in ["", "Helios", "a..b", "a-", ".a", "a b", "a_b"] {
            assert!(!valid_id(bad), "{bad}");
        }
        assert!(valid_name("retouch"));
        assert!(valid_name("in_0"));
        for bad in ["", "0a", "a.b", "A", "@", "a-b"] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert_eq!(slug("helios.portrait-retouch"), "helios-portrait-retouch");
    }

    #[test]
    fn unknown_fields_are_refused() {
        let e = serde_json::from_str::<Tools>(r#"{ "toolbox": [], "colour": 1 }"#).unwrap_err();
        assert!(e.to_string().contains("colour"), "{e}");
    }
}
