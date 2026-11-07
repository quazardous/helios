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

/// The library a template works with: where it imports from, which
/// images it shows, and how culling goes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryDesc {
    /// The import source the workspace offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportDesc>,
    /// The library opens filtered on these images (the picks only, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<QueryDesc>,
    /// Culling moves to the next image after each mark.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_advance: bool,
    /// The export's settings, ready in the export dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<ExportDesc>,
}

/// How the workspace exports its images, written as `library.export`
/// takes its arguments: what the export dialog starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportDesc {
    /// `jpeg`, `png` or `tiff` ([`EXPORT_FORMATS`]).
    #[serde(default = "jpeg")]
    pub format: String,
    /// JPEG's quality, 1 to 100 (92 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    /// PNG's bits per channel, 8 or 16 (8 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u8>,
    /// The longer side at most, in pixels; the full size when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_edge: Option<u32>,
    /// The colour space written ([`EXPORT_SPACES`]).
    #[serde(default = "srgb")]
    pub space: String,
    /// The folder written to: absolute, or under the home folder
    /// (`~/Pictures/Export`); asked when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The file names, as an import's renaming writes them (`{stem}`,
    /// `{date}-{seq}`); the extension follows the format.
    #[serde(default = "stem")]
    pub pattern: String,
    /// A name taken already: `number` (another name), `overwrite` or
    /// `skip` ([`CLASHES`]).
    #[serde(default = "number")]
    pub clash: String,
}

impl Default for ExportDesc {
    fn default() -> Self {
        ExportDesc {
            format: jpeg(),
            quality: None,
            bits: None,
            long_edge: None,
            space: srgb(),
            folder: None,
            pattern: stem(),
            clash: number(),
        }
    }
}

fn jpeg() -> String {
    "jpeg".into()
}

fn srgb() -> String {
    "srgb".into()
}

fn stem() -> String {
    "{stem}".into()
}

fn number() -> String {
    "number".into()
}

/// The formats an export may write.
pub const EXPORT_FORMATS: [&str; 3] = ["jpeg", "png", "tiff"];
/// The colour spaces an export may write in.
pub const EXPORT_SPACES: [&str; 4] = ["srgb", "display-p3", "adobe-rgb", "rec2020"];
/// What an export may do when a file name is taken.
pub const CLASHES: [&str; 3] = ["number", "overwrite", "skip"];

/// Which images of the library a workspace shows, as the search field's
/// criteria: all must hold (labels and flags: any of them; tags: all of
/// them).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryDesc {
    /// At least this many stars (0 to 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_rating: Option<u8>,
    /// One of these colour labels ([`LABELS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// One of these flags ([`FLAGS`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    /// All of these tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl QueryDesc {
    /// Whether it filters nothing.
    pub fn is_empty(&self) -> bool {
        self.min_rating.is_none()
            && self.labels.is_empty()
            && self.flags.is_empty()
            && self.tags.is_empty()
    }

    /// As typed in the search field: `rating:3 label:red flag:pick
    /// tag:sea`.
    pub fn search_text(&self) -> String {
        let mut words = Vec::new();
        if let Some(r) = self.min_rating {
            words.push(format!("rating:{r}"));
        }
        words.extend(self.labels.iter().map(|l| format!("label:{l}")));
        words.extend(self.flags.iter().map(|f| format!("flag:{f}")));
        words.extend(self.tags.iter().map(|t| format!("tag:{t}")));
        words.join(" ")
    }
}

/// The most stars a query may ask for.
pub const MAX_RATING: u8 = 5;
/// The colour labels a query may name.
pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];
/// The flags a query may name: picked, rejected, or neither.
pub const FLAGS: [&str; 3] = ["pick", "reject", "none"];

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
    /// Keep each photo's darktable sidecars (`.xmp`, its base and its
    /// duplicates) as versions of it; on unless said otherwise.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub darktable: bool,
}

fn yes() -> bool {
    true
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "serde's signature")]
fn is_yes(on: &bool) -> bool {
    *on
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
    /// The sources given a raw file's sensor data only (see
    /// [`SourceKind::Raw`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw_sources: Vec<String>,
    /// The sources given an image, a picture or a raw (see
    /// [`SourceKind::Image`]); the sources in neither list are given a
    /// picture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_sources: Vec<String>,
    pub nodes: Vec<NodeDesc>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// The output the viewport shows.
    pub view: Endpoint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Preset>,
}

impl NetworkDesc {
    /// What the source `name` is given.
    pub fn source_kind(&self, name: &str) -> SourceKind {
        if self.raw_sources.iter().any(|s| s == name) {
            SourceKind::Raw
        } else if self.image_sources.iter().any(|s| s == name) {
            SourceKind::Image
        } else {
            SourceKind::Picture
        }
    }

    /// Whether a source takes a raw file's sensor data only: the network
    /// refuses a picture.
    pub fn reads_raw(&self) -> bool {
        self.sources
            .iter()
            .any(|s| self.source_kind(s) == SourceKind::Raw)
    }

    /// Whether a raw file goes through the network as its sensor data:
    /// a source takes a raw (a raw source, or an image source, which
    /// puts the raw front before it).
    pub fn takes_raw(&self) -> bool {
        self.sources
            .iter()
            .any(|s| self.source_kind(s) != SourceKind::Picture)
    }
}

/// What a source of a template's network is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A picture in the working space, four channels (RGBA): a JPEG, a
    /// PNG, a TIFF, or a raw file's embedded preview.
    Picture,
    /// A raw file's sensor mosaic, one channel in raw units, with the
    /// camera's description (black and white levels, filter pattern,
    /// white balance coefficients, matrix) as the context of every node
    /// reading it, directly or not.
    Raw,
    /// An image: a picture goes on as a picture source's; a raw file's
    /// mosaic, with its camera, goes through the raw front
    /// ([`RAW_FRONT`]: levels, white balance, highlights, demosaicing,
    /// camera colours) first, so the nodes reading the source get a
    /// picture in the working space either way.
    Image,
}

impl SourceKind {
    /// The channels the nodes reading such a source get: one for a
    /// raw's mosaic, four (RGBA) for a picture or an image.
    pub fn channels(self) -> u32 {
        match self {
            SourceKind::Raw => 1,
            SourceKind::Picture | SourceKind::Image => 4,
        }
    }
}

/// The composite an image source puts before what reads it when it is
/// given a raw: the raw chain up to the working space.
pub const RAW_FRONT: (&str, u32) = ("helios.raw-front", 1);

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
    /// Put over `params` when the network is built with a raw (a raw
    /// source, or an image source given one): a raw's defaults, as
    /// darktable's for its exposure (+0.7 EV, the camera's bias taken
    /// back). Checked against the manifest as `params` are.
    #[serde(default, skip_serializing_if = "is_empty")]
    pub raw_params: Params,
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
    /// What makes it a workshop (experimental): its intent and the
    /// relative targets of its exposed parameters. Read whether
    /// workshops are switched on or not; off, it is the macro it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop: Option<Workshop>,
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

/// What makes a macro a workshop (experimental, see
/// `docs/design/workshops.md`): what it is for, in the user's words, and
/// the relative targets its parameters are aimed at on each image.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workshop {
    /// What it is for, in English ("Brighter midtones without losing the
    /// highlights").
    pub intent: String,
    /// English words it is found by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// At most one per exposed number parameter, solved in this order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<Target>,
}

/// What the image should measure, and which exposed parameter is moved
/// to get there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// The exposed parameter's name.
    pub param: String,
    pub measure: Measure,
    /// In the measure's unit ([`Measure::unit`]); with `at: input`, an
    /// offset from what the input measures.
    pub value: f64,
    #[serde(default, skip_serializing_if = "At::is_output")]
    pub at: At,
}

/// What a target measures, on the luminance of the working data (linear
/// Rec.2020).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Measure {
    /// The median luminance, in stops around 1.0.
    #[serde(rename = "luma.median")]
    LumaMedian,
    /// The 99th percentile of luminance, in stops around 1.0.
    #[serde(rename = "luma.high")]
    LumaHigh,
    /// The share of pixels above 1.0, in %.
    #[serde(rename = "clip.high")]
    ClipHigh,
}

impl Measure {
    /// Every measure.
    pub const ALL: [Measure; 3] = [Measure::LumaMedian, Measure::LumaHigh, Measure::ClipHigh];

    /// Its name in a file.
    pub fn name(self) -> &'static str {
        match self {
            Measure::LumaMedian => "luma.median",
            Measure::LumaHigh => "luma.high",
            Measure::ClipHigh => "clip.high",
        }
    }

    /// The unit of a target's value.
    pub fn unit(self) -> &'static str {
        match self {
            Measure::LumaMedian | Measure::LumaHigh => "stops",
            Measure::ClipHigh => "%",
        }
    }
}

/// Where a target is measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum At {
    /// The workshop's output.
    #[default]
    Output,
    /// What enters it: the target is an offset from what the input
    /// measures.
    Input,
}

impl At {
    #[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if
    fn is_output(&self) -> bool {
        *self == At::Output
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

/// An inner parameter made the composite's own, and how it is shown:
/// every field after `param` is optional, the inner parameter's own
/// otherwise. A name repeated on several inner nodes links them; the
/// interface is given on its first entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exposed {
    pub name: String,
    pub node: String,
    pub param: String,
    /// A short label, in English; the name otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The unit shown (`EV`, `%`, `px`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// The section it is shown under; sections keep the order of their
    /// first parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// A narrower hard range than the inner one (a number or a list of
    /// numbers), never wider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// The range a slider spans by default, within the hard range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft: Option<[f64; 2]>,
    /// The options a choice keeps, in the order shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// The default, tagged by kind as a node's parameters; the inner
    /// node's value otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<helios_core::Value>,
}

impl Exposed {
    /// How the macro shows it.
    pub fn exposure(&self) -> helios_core::Exposure {
        helios_core::Exposure {
            label: self.label.clone(),
            description: self.description.clone(),
            unit: self.unit.clone(),
            group: self.group.clone(),
            min: self.min,
            max: self.max,
            soft: self.soft.map(|[lo, hi]| (lo, hi)),
            options: self.options.clone(),
            default: self.default.clone(),
        }
    }
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
