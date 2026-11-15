//! Declarations: the keys an extractor gives, its reusable definition,
//! and a use of that definition attached to a carrier node.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// Values a finite key may not declare: the schema and the consumers
/// give them a meaning of their own.
pub const RESERVED_VALUES: [&str; 2] = ["none", "other"];

/// The type of a key's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyType {
    /// Free text (a caption, a title).
    Text,
    /// A short label (`dog`, `Canon EOS R5`), optionally with a confidence.
    Label,
    /// A real number, optionally bounded.
    Number,
    /// A whole number, optionally bounded.
    Integer,
    /// Yes or no.
    Bool,
}

impl KeyType {
    /// Its name in declarations.
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Label => "label",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Bool => "bool",
        }
    }

    fn is_textual(self) -> bool {
        matches!(self, Self::Text | Self::Label)
    }

    fn is_numeric(self) -> bool {
        matches!(self, Self::Number | Self::Integer)
    }
}

/// Which values a key can take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Values {
    /// Only these (an enum): a consumer can list every case.
    Finite(Vec<String>),
    /// These are expected, others may come (they are kept and counted
    /// as new): a consumer lists these plus what a preview run saw.
    SemiFinite(Vec<String>),
    /// Any value.
    Open,
}

impl Values {
    /// The declared list, empty for an open key.
    pub fn list(&self) -> &[String] {
        match self {
            Self::Finite(v) | Self::SemiFinite(v) => v,
            Self::Open => &[],
        }
    }
}

/// One key an extractor declares.
///
/// In a manifest, `values = [...]` makes the key finite, adding
/// `semi_finite = true` makes the list a seed, no `values` leaves it open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawKey", into = "RawKey")]
pub struct KeyDecl {
    /// Lowercase, unique within the extractor; consumers see
    /// `<extractor>.<name>`.
    pub name: String,
    pub ty: KeyType,
    pub values: Values,
    /// Several values (a list) instead of at most one.
    pub multi: bool,
    /// Each value carries a confidence `p` in `[0, 1]` (labels only).
    pub confidence: bool,
    /// A finite key accepts `other`, and a value outside the list is
    /// mapped to it instead of dropped.
    pub other: bool,
    /// Bounds of a number or integer key.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The unit of a number key, for display (`mm`, `s`).
    pub unit: Option<String>,
}

impl KeyDecl {
    /// An open, single-valued key of this type.
    pub fn new(name: &str, ty: KeyType) -> Self {
        Self {
            name: name.to_owned(),
            ty,
            values: Values::Open,
            multi: false,
            confidence: false,
            other: false,
            min: None,
            max: None,
            unit: None,
        }
    }

    /// This key with a finite list of values.
    pub fn finite(mut self, values: &[&str]) -> Self {
        self.values = Values::Finite(values.iter().map(|v| (*v).to_owned()).collect());
        self
    }

    /// This key with a semi-finite list of values.
    pub fn semi_finite(mut self, values: &[&str]) -> Self {
        self.values = Values::SemiFinite(values.iter().map(|v| (*v).to_owned()).collect());
        self
    }

    /// This key multi-valued.
    pub fn multi(mut self) -> Self {
        self.multi = true;
        self
    }

    /// This key with a confidence per value.
    pub fn with_confidence(mut self) -> Self {
        self.confidence = true;
        self
    }

    /// This finite key accepting `other`.
    pub fn with_other(mut self) -> Self {
        self.other = true;
        self
    }

    /// This number key bounded.
    pub fn range(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// What is wrong with this declaration; empty when it is sound.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let at = format!("key `{}`", self.name);
        if !is_name(&self.name) {
            out.push(format!(
                "{at}: a key name is lowercase letters, digits and `_`, starting with a letter"
            ));
        }
        let list = self.values.list();
        if !matches!(self.values, Values::Open) {
            if !self.ty.is_textual() {
                out.push(format!(
                    "{at}: only text and label keys list their values, not {}",
                    self.ty.name()
                ));
            }
            if list.is_empty() {
                out.push(format!(
                    "{at}: a finite or semi-finite key lists at least one value"
                ));
            }
            let mut seen = BTreeSet::new();
            for v in list {
                if v.trim().is_empty() {
                    out.push(format!("{at}: an empty value"));
                } else if RESERVED_VALUES.contains(&v.as_str()) {
                    out.push(format!("{at}: `{v}` is reserved"));
                } else if !seen.insert(v.as_str()) {
                    out.push(format!("{at}: `{v}` is listed twice"));
                }
            }
        }
        if self.other && !matches!(self.values, Values::Finite(_)) {
            out.push(format!("{at}: `other` is for finite keys"));
        }
        if self.confidence && self.ty != KeyType::Label {
            out.push(format!("{at}: a confidence is for label keys"));
        }
        if (self.min.is_some() || self.max.is_some() || self.unit.is_some())
            && !self.ty.is_numeric()
        {
            out.push(format!("{at}: bounds and a unit are for number keys"));
        }
        if let (Some(lo), Some(hi)) = (self.min, self.max)
            && lo > hi
        {
            out.push(format!("{at}: min {lo} is above max {hi}"));
        }
        out
    }
}

// The manifest's flags, one per declared property.
#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKey {
    name: String,
    #[serde(rename = "type")]
    ty: KeyType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_false")]
    semi_finite: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    multi: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    confidence: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    other: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if
fn is_false(b: &bool) -> bool {
    !*b
}

impl TryFrom<RawKey> for KeyDecl {
    type Error = String;

    fn try_from(raw: RawKey) -> Result<Self, String> {
        let values = match (raw.values, raw.semi_finite) {
            (Some(v), false) => Values::Finite(v),
            (Some(v), true) => Values::SemiFinite(v),
            (None, false) => Values::Open,
            (None, true) => {
                return Err(format!(
                    "key `{}`: `semi_finite` needs a `values` list",
                    raw.name
                ));
            }
        };
        Ok(Self {
            name: raw.name,
            ty: raw.ty,
            values,
            multi: raw.multi,
            confidence: raw.confidence,
            other: raw.other,
            min: raw.min,
            max: raw.max,
            unit: raw.unit,
        })
    }
}

impl From<KeyDecl> for RawKey {
    fn from(k: KeyDecl) -> Self {
        let (values, semi_finite) = match k.values {
            Values::Finite(v) => (Some(v), false),
            Values::SemiFinite(v) => (Some(v), true),
            Values::Open => (None, false),
        };
        Self {
            name: k.name,
            ty: k.ty,
            values,
            semi_finite,
            multi: k.multi,
            confidence: k.confidence,
            other: k.other,
            min: k.min,
            max: k.max,
            unit: k.unit,
        }
    }
}

/// What a model needs to answer: the image, or texts only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// The image is sent (helios-ai's `vision`).
    Vision,
    /// Texts only, far cheaper (helios-ai's `chat`).
    Chat,
}

/// A variable filled per image in an LLM extractor's prompt, written
/// `{{name}}` in its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextVar {
    /// The photo's keywords.
    Keywords,
    /// Its caption, accepted or suggested.
    Caption,
    /// Camera, lens, date and settings.
    Exif,
    /// The folder it is in.
    Folder,
    /// The import session's name and tags.
    Import,
    /// A gather node's value.
    Context,
    /// The language to answer in.
    Language,
}

impl ContextVar {
    /// Every variable.
    pub const ALL: [Self; 7] = [
        Self::Keywords,
        Self::Caption,
        Self::Exif,
        Self::Folder,
        Self::Import,
        Self::Context,
        Self::Language,
    ];

    /// Its name in a prompt.
    pub fn name(self) -> &'static str {
        match self {
            Self::Keywords => "keywords",
            Self::Caption => "caption",
            Self::Exif => "exif",
            Self::Folder => "folder",
            Self::Import => "import",
            Self::Context => "context",
            Self::Language => "language",
        }
    }
}

/// How an LLM extractor asks: its prompt and the capability it needs.
/// It never names a provider unless the user pins one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmSpec {
    pub capability: Capability,
    /// The user's text, in plain words. Exactly one of `prompt` and
    /// `prompt_id` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// A helios-ai prompt asset, `id@version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    /// The variables the prompt uses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContextVar>,
    /// An optional provider override, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// An optional model override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// An extractor computed in code (or by a plugin) with fixed keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinSpec {
    /// The computation's name, as the module providing it registers it
    /// (`capture`, `cull`).
    pub compute: String,
}

/// How an extractor computes its keys. In a manifest, the `[llm]` or
/// `[builtin]` table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Llm(LlmSpec),
    Builtin(BuiltinSpec),
}

/// A reusable extractor: made in the extractor editor or shipped by a
/// module. Any change of how it computes or of its keys is a new
/// version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractorDef {
    /// Dotted lowercase, unique in the catalogue (`animals`, `exif`).
    pub id: String,
    pub version: u32,
    /// Its name for people.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Data kept on the photo, or used within the run only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub keep: bool,
    /// Confidences below it are dropped (and counted).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub confidence_floor: f64,
    #[serde(flatten)]
    pub kind: Kind,
    pub keys: Vec<KeyDecl>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if
fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

impl ExtractorDef {
    /// `id@version`, the pinned name.
    pub fn pinned(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    /// A key by name.
    pub fn key(&self, name: &str) -> Option<&KeyDecl> {
        self.keys.iter().find(|k| k.name == name)
    }

    /// What is wrong with this definition; empty when it is sound.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.id.split('.').all(is_name) {
            out.push(format!(
                "id `{}`: dotted parts of lowercase letters, digits and `_`",
                self.id
            ));
        }
        if self.version == 0 {
            out.push("version starts at 1".to_owned());
        }
        if self.title.trim().is_empty() {
            out.push("a title is needed".to_owned());
        }
        if !(0.0..=1.0).contains(&self.confidence_floor) {
            out.push("confidence_floor is within [0, 1]".to_owned());
        }
        if self.keys.is_empty() {
            out.push("an extractor declares at least one key".to_owned());
        }
        let mut names = BTreeSet::new();
        for k in &self.keys {
            if !names.insert(k.name.as_str()) {
                out.push(format!("key `{}` is declared twice", k.name));
            }
            out.extend(k.problems());
        }
        match &self.kind {
            Kind::Llm(llm) => match (&llm.prompt, &llm.prompt_id) {
                (Some(text), None) => {
                    if text.trim().is_empty() {
                        out.push("the prompt is empty".to_owned());
                    }
                    for var in placeholders(text) {
                        if !llm.context.iter().any(|c| c.name() == var) {
                            out.push(format!(
                                "the prompt uses `{{{{{var}}}}}`, not in its context"
                            ));
                        }
                    }
                }
                (None, Some(id)) => {
                    let ok = id
                        .split_once('@')
                        .is_some_and(|(n, v)| !n.is_empty() && v.parse::<u32>().is_ok());
                    if !ok {
                        out.push(format!("prompt_id `{id}` is `id@version`"));
                    }
                }
                _ => out.push("an LLM extractor has either `prompt` or `prompt_id`".to_owned()),
            },
            Kind::Builtin(b) => {
                if b.compute.trim().is_empty() {
                    out.push("a built-in extractor names its computation".to_owned());
                }
            }
        }
        out
    }
}

/// The `{{name}}` placeholders of a prompt text.
fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A definition attached to a carrier node (a router, a filter, an
/// updater). One definition may have several uses in a diagram; they
/// share its cached results when their parameters match.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractorUse {
    /// The definition's id.
    pub def: String,
    /// The version pinned: a new version of the definition does not
    /// change a diagram until its use is moved to it.
    pub version: u32,
    /// Per-use parameters (`language`, a threshold...), part of the
    /// cache key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
    /// Keeps the results on the photo (`true`) or within the run
    /// (`false`) instead of what the definition says. Not part of the
    /// cache key: where a result is stored does not change it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<bool>,
}

impl ExtractorUse {
    /// A use of this definition, pinned to its version, no parameters.
    pub fn of(def: &ExtractorDef) -> Self {
        Self {
            def: def.id.clone(),
            version: def.version,
            params: BTreeMap::new(),
            keep: None,
        }
    }

    /// Whether this use keeps its results on the photo: its own word,
    /// else the definition's.
    pub fn keeps(&self, def: &ExtractorDef) -> bool {
        self.keep.unwrap_or(def.keep)
    }
}

/// Why a declaration or a use cannot be taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclError {
    /// The manifest does not parse.
    Parse(String),
    /// A definition is unsound: its pinned name and the problems.
    Invalid(String, Vec<String>),
    /// The same `id@version` twice.
    Duplicate(String),
    /// A use names a definition the catalogue lacks.
    Unknown(String),
}

impl fmt::Display for DeclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "extractor manifest: {e}"),
            Self::Invalid(id, problems) => write!(f, "extractor {id}: {}", problems.join("; ")),
            Self::Duplicate(id) => write!(f, "extractor {id} is declared twice"),
            Self::Unknown(id) => write!(f, "no extractor {id}"),
        }
    }
}

impl std::error::Error for DeclError {}

/// The `[[extractors]]` a module or plugin manifest ships, or a file of
/// user-made definitions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub extractors: Vec<ExtractorDef>,
}

impl Manifest {
    /// A manifest from TOML, every definition checked.
    pub fn from_toml(text: &str) -> Result<Self, DeclError> {
        let m: Self = toml::from_str(text).map_err(|e| DeclError::Parse(e.to_string()))?;
        m.checked()
    }

    /// A manifest from JSON, every definition checked.
    pub fn from_json(text: &str) -> Result<Self, DeclError> {
        let m: Self = serde_json::from_str(text).map_err(|e| DeclError::Parse(e.to_string()))?;
        m.checked()
    }

    /// As TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a manifest serializes to TOML")
    }

    fn checked(self) -> Result<Self, DeclError> {
        for d in &self.extractors {
            let problems = d.problems();
            if !problems.is_empty() {
                return Err(DeclError::Invalid(d.pinned(), problems));
            }
        }
        Ok(self)
    }
}

/// The definitions known: built-ins, modules' and the user's.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    defs: BTreeMap<(String, u32), ExtractorDef>,
}

impl Catalog {
    /// Adds a definition, refused when unsound or already there.
    pub fn add(&mut self, def: ExtractorDef) -> Result<(), DeclError> {
        let problems = def.problems();
        if !problems.is_empty() {
            return Err(DeclError::Invalid(def.pinned(), problems));
        }
        let key = (def.id.clone(), def.version);
        if self.defs.contains_key(&key) {
            return Err(DeclError::Duplicate(def.pinned()));
        }
        self.defs.insert(key, def);
        Ok(())
    }

    /// A definition by id and version.
    pub fn get(&self, id: &str, version: u32) -> Option<&ExtractorDef> {
        self.defs.get(&(id.to_owned(), version))
    }

    /// The latest version of a definition.
    pub fn latest(&self, id: &str) -> Option<&ExtractorDef> {
        self.defs
            .range((id.to_owned(), 0)..=(id.to_owned(), u32::MAX))
            .next_back()
            .map(|(_, d)| d)
    }

    /// The definition a use points at.
    pub fn resolve(&self, u: &ExtractorUse) -> Result<&ExtractorDef, DeclError> {
        self.get(&u.def, u.version)
            .ok_or_else(|| DeclError::Unknown(format!("{}@{}", u.def, u.version)))
    }

    /// Every definition, by id then version.
    pub fn iter(&self) -> impl Iterator<Item = &ExtractorDef> {
        self.defs.values()
    }
}
