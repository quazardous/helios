//! The configuration contract: what a plugin or a module lets the person
//! configure, declared once as data, at three levels ([`Level`]): the
//! plugin or module itself, and each account and each source it creates.
//! Settings and the import window render it, the values are checked
//! against it, and a plugin's manifest declares it as
//! `[settings.<key>]`, `[account.settings.<key>]` and
//! `[source.settings.<key>]`:
//!
//! ```toml
//! [settings.quality]
//! type = "int"
//! default = 90
//! min = 1
//! max = 100
//! label = "Quality"
//! help = "The JPEG quality of the previews it asks for"
//! group = "Previews"
//!
//! [account.settings.token]
//! type = "secret"
//! label = "Access token"
//!
//! [source.settings.into]
//! type = "path"
//! path = "folder"
//! label = "Into"
//! visible_when = { copy = true }
//! ```
//!
//! Types ([`KeyType`]): `bool`, `int` and `float` (with `min`, `max` and
//! `step`), `string`, `choice` (one of `choices`), `path` (`path = "file"`
//! or `"folder"`) and `secret`, which is kept in the system keyring and
//! never shown back, so it has no default. Each key has an English label
//! and help, translated in its owner's catalogue ([`message`]), an
//! optional group (an English heading), an `order` among its group, and
//! `visible_when`: the keys and values that show it (a list giving the
//! values any of which does).
//!
//! A contract produces a JSON Schema ([`Contract::schema`]) and values are
//! checked against it with the same checker as the node's command params
//! ([`crate::schema`]). [`Contract::problems`] lists what is wrong with a
//! declaration, a default that does not fit its key among them; a plugin
//! with any is refused when its manifest is read.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};

use crate::schema;

/// What a key holds.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeyType {
    Bool,
    Int,
    Float,
    #[default]
    String,
    /// One of the key's `choices`.
    Choice,
    /// A file or a folder on this machine (`path`).
    Path,
    /// A password, key or token: kept in the keyring, never shown back.
    Secret,
}

impl KeyType {
    fn name(self) -> &'static str {
        match self {
            KeyType::Bool => "bool",
            KeyType::Int => "int",
            KeyType::Float => "float",
            KeyType::String => "string",
            KeyType::Choice => "choice",
            KeyType::Path => "path",
            KeyType::Secret => "secret",
        }
    }

    fn numeric(self) -> bool {
        matches!(self, KeyType::Int | KeyType::Float)
    }
}

/// What a `path` key picks.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    File,
    Folder,
}

/// One key of a contract.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConfigKey {
    #[serde(rename = "type")]
    pub ty: KeyType,
    /// Its value until the person sets one; none: unset. A secret has
    /// none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Json>,
    /// Its name in the interface, in English; the key's name when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// What it does, in a sentence, in English (`about` in older
    /// manifests).
    #[serde(default, alias = "about", skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// The heading it is shown under, in English; none: the first group.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Its place among its group's keys, then by name.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub order: i32,
    /// A number's bounds and the step a control moves it by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// The values a `choice` may take.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// What a `path` picks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathKind>,
    /// Shown only while each key named has the value given (one of them,
    /// for a list).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub visible_when: BTreeMap<String, Json>,
    /// Its value may differ per library: Settings offers "Set for: This
    /// library" beside the app's value. A plugin's own keys only; never
    /// a secret.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub per_library: bool,
    /// A change of its value restarts the plugin, which reads it only as
    /// it starts; without, the running plugin is told the new values
    /// (`settings/changed`). A plugin's own keys only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restart: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if hands a reference"
)]
fn is_zero(n: &i32) -> bool {
    *n == 0
}

impl ConfigKey {
    /// A key of this type, without default, label or bounds.
    pub fn new(ty: KeyType) -> Self {
        Self {
            ty,
            ..Self::default()
        }
    }

    pub fn default_value(mut self, value: Json) -> Self {
        self.default = Some(value);
        self
    }

    pub fn label(mut self, label: &str) -> Self {
        label.clone_into(&mut self.label);
        self
    }

    pub fn help(mut self, help: &str) -> Self {
        help.clone_into(&mut self.help);
        self
    }

    pub fn group(mut self, group: &str) -> Self {
        group.clone_into(&mut self.group);
        self
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn range(mut self, min: f64, max: f64, step: f64) -> Self {
        (self.min, self.max, self.step) = (Some(min), Some(max), Some(step));
        self
    }

    pub fn choices(mut self, choices: &[&str]) -> Self {
        self.choices = choices.iter().map(|c| (*c).to_owned()).collect();
        self
    }

    pub fn path(mut self, kind: PathKind) -> Self {
        self.path = Some(kind);
        self
    }

    pub fn per_library(mut self) -> Self {
        self.per_library = true;
        self
    }

    pub fn restart(mut self) -> Self {
        self.restart = true;
        self
    }

    pub fn visible_when(mut self, key: &str, value: Json) -> Self {
        self.visible_when.insert(key.to_owned(), value);
        self
    }

    /// The JSON Schema of its value.
    pub fn schema(&self) -> Json {
        let mut s = match self.ty {
            KeyType::Bool => json!({ "type": "boolean" }),
            KeyType::Int => json!({ "type": "integer" }),
            KeyType::Float => json!({ "type": "number" }),
            KeyType::String => json!({ "type": "string" }),
            KeyType::Choice => json!({ "type": "string", "enum": self.choices }),
            KeyType::Path => json!({
                "type": "string",
                "format": match self.path {
                    Some(PathKind::File) => "file-path",
                    _ => "folder-path",
                },
            }),
            KeyType::Secret => json!({ "type": "string", "writeOnly": true }),
        };
        if self.ty.numeric() {
            if let Some(min) = self.min {
                s["minimum"] = json!(min);
            }
            if let Some(max) = self.max {
                s["maximum"] = json!(max);
            }
        }
        if !self.label.is_empty() {
            s["title"] = json!(self.label);
        }
        if !self.help.is_empty() {
            s["description"] = json!(self.help);
        }
        if let Some(default) = &self.default
            && self.ty != KeyType::Secret
        {
            s["default"] = default.clone();
        }
        s
    }

    /// Whether a value fits the key: its type, its bounds, its choices.
    pub fn check(&self, value: &Json) -> Result<(), String> {
        schema::validate_at(&self.schema(), value, "value")
            .map_err(|e| e.trim_start_matches("value: ").to_owned())
    }

    /// What is wrong with the declaration of the key `name`, the other
    /// keys of its contract being `all`.
    fn problems(&self, name: &str, all: &Contract) -> Vec<String> {
        let mut p = Vec::new();
        let ty = self.ty.name();
        if !self.ty.numeric() && (self.min.is_some() || self.max.is_some() || self.step.is_some()) {
            p.push(format!(
                "`min`, `max` and `step` are for a number, not a {ty}"
            ));
        }
        if let (Some(min), Some(max)) = (self.min, self.max)
            && min > max
        {
            p.push(format!("`min` {min} is above `max` {max}"));
        }
        if self.step.is_some_and(|s| s <= 0.0) {
            p.push("`step` must be above 0".into());
        }
        if self.ty == KeyType::Int
            && [self.min, self.max, self.step]
                .into_iter()
                .flatten()
                .any(|n| n.fract() != 0.0)
        {
            p.push("an int's `min`, `max` and `step` are whole numbers".into());
        }
        match (self.ty, self.choices.is_empty()) {
            (KeyType::Choice, true) => p.push("a choice without `choices`".into()),
            (KeyType::Choice, false) | (_, true) => {}
            (_, false) => p.push(format!("`choices` are for a choice, not a {ty}")),
        }
        match (self.ty, self.path) {
            (KeyType::Path, None) => {
                p.push("a path without `path = \"file\"` or `\"folder\"`".into());
            }
            (KeyType::Path, Some(_)) | (_, None) => {}
            (_, Some(_)) => p.push(format!("`path` is for a path, not a {ty}")),
        }
        match (&self.default, self.ty) {
            (Some(_), KeyType::Secret) => {
                p.push("a secret has no default: it would be written in the clear".into());
            }
            (Some(d), _) => {
                if let Err(e) = self.check(d) {
                    p.push(format!("default {d}: {e}"));
                }
            }
            (None, _) => {}
        }
        if self.per_library && self.ty == KeyType::Secret {
            p.push("a secret is kept in the keyring, not per library".into());
        }
        for (other, values) in &self.visible_when {
            let Some(key) = all.keys.get(other) else {
                p.push(format!(
                    "`visible_when` names `{other}`, which it does not declare"
                ));
                continue;
            };
            if other == name {
                p.push("`visible_when` names the key itself".into());
                continue;
            }
            if key.ty == KeyType::Secret {
                p.push(format!("`visible_when` names the secret `{other}`"));
                continue;
            }
            let many = match values {
                Json::Array(many) => many.clone(),
                one => vec![one.clone()],
            };
            for v in many {
                if let Err(e) = key.check(&v) {
                    p.push(format!("`visible_when` {other} = {v}: {e}"));
                }
            }
        }
        p.into_iter()
            .map(|e| format!("setting `{name}`: {e}"))
            .collect()
    }
}

/// The keys one level declares, by name.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(transparent)]
pub struct Contract {
    pub keys: BTreeMap<String, ConfigKey>,
}

impl Contract {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a key, for the contracts declared in code.
    pub fn key(mut self, name: &str, key: ConfigKey) -> Self {
        self.keys.insert(name.to_owned(), key);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&ConfigKey> {
        self.keys.get(name)
    }

    /// The keys in the order shown: by group, the groups in the order
    /// their first key comes, then by `order` and name.
    pub fn ordered(&self) -> Vec<(&str, &ConfigKey)> {
        let mut keys: Vec<(&str, &ConfigKey)> =
            self.keys.iter().map(|(n, k)| (n.as_str(), k)).collect();
        keys.sort_by(|a, b| (a.1.order, a.0).cmp(&(b.1.order, b.0)));
        let mut groups: Vec<&str> = Vec::new();
        for (_, k) in &keys {
            if !groups.contains(&k.group.as_str()) {
                groups.push(&k.group);
            }
        }
        keys.sort_by_key(|(_, k)| groups.iter().position(|g| *g == k.group));
        keys
    }

    /// What is wrong with the declaration; empty: it is sound.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        for (name, key) in &self.keys {
            let sound = !name.is_empty()
                && name.starts_with(|c: char| c.is_ascii_lowercase())
                && name.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_')
                });
            if !sound {
                p.push(format!(
                    "setting `{name}`: a name of lowercase letters, digits, `-` and `_`"
                ));
            }
            p.extend(key.problems(name, self));
        }
        p
    }

    /// The JSON Schema of its values: an object of its keys, none
    /// required, no other taken.
    pub fn schema(&self) -> Json {
        let props: Map<String, Json> = self
            .keys
            .iter()
            .map(|(n, k)| (n.clone(), k.schema()))
            .collect();
        json!({
            "type": "object",
            "properties": props,
            "additionalProperties": false,
        })
    }

    /// Checks values against the contract; the first mismatch, named by
    /// its path (`settings.quality`).
    pub fn check(&self, values: &Json) -> Result<(), String> {
        schema::validate_at(&self.schema(), values, "settings")
    }

    /// The defaults, by key; secrets and keys without one left out.
    pub fn defaults(&self) -> Map<String, Json> {
        self.keys
            .iter()
            .filter(|(_, k)| k.ty != KeyType::Secret)
            .filter_map(|(n, k)| Some((n.clone(), k.default.clone()?)))
            .collect()
    }

    /// The values a plugin is handed: the defaults, under the values
    /// `given` (secrets among them) that fit their key. A value of a key
    /// the contract does not declare, or one that does not fit its key,
    /// is left out, with its reason in the list returned; a reason never
    /// holds the value.
    pub fn resolve(&self, given: &Map<String, Json>) -> (Map<String, Json>, Vec<String>) {
        let mut out = self.defaults();
        let mut refused = Vec::new();
        for (name, v) in given {
            match self.keys.get(name) {
                None => refused.push(format!("`{name}`: not declared; left out")),
                Some(key) => match key.check(v) {
                    Ok(()) => {
                        out.insert(name.clone(), v.clone());
                    }
                    Err(_) if key.ty == KeyType::Secret => {
                        refused.push(format!("`{name}`: a secret is a text; left out"));
                    }
                    Err(e) => refused.push(format!("`{name}`: {e}; left out")),
                },
            }
        }
        (out, refused)
    }

    /// The keys that are secrets.
    pub fn secrets(&self) -> impl Iterator<Item = &str> {
        self.keys
            .iter()
            .filter(|(_, k)| k.ty == KeyType::Secret)
            .map(|(n, _)| n.as_str())
    }

    /// Whether the key `name` is shown with these values (its defaults
    /// filling in those missing).
    pub fn visible(&self, name: &str, values: &Map<String, Json>) -> bool {
        let Some(key) = self.keys.get(name) else {
            return false;
        };
        let defaults = self.defaults();
        key.visible_when.iter().all(|(other, wanted)| {
            let Some(v) = values.get(other).or_else(|| defaults.get(other)) else {
                return false;
            };
            match wanted {
                Json::Array(any) => any.contains(v),
                one => one == v,
            }
        })
    }
}

/// Who a contract configures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// The plugin, module or connector itself.
    Own,
    /// Each account it connects.
    Account,
    /// Each source it reads.
    Source,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Own, Level::Account, Level::Source];

    /// Its prefix in a translation message.
    fn prefix(self) -> &'static str {
        match self {
            Level::Own => "setting",
            Level::Account => "account-setting",
            Level::Source => "source-setting",
        }
    }
}

/// The contracts of a plugin, module or connector, one per level.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Contracts {
    #[serde(skip_serializing_if = "Contract::is_empty")]
    pub own: Contract,
    #[serde(skip_serializing_if = "Contract::is_empty")]
    pub account: Contract,
    #[serde(skip_serializing_if = "Contract::is_empty")]
    pub source: Contract,
}

impl Contracts {
    pub fn get(&self, level: Level) -> &Contract {
        match level {
            Level::Own => &self.own,
            Level::Account => &self.account,
            Level::Source => &self.source,
        }
    }

    /// What is wrong with any level, each named by its manifest table.
    pub fn problems(&self) -> Vec<String> {
        Level::ALL
            .into_iter()
            .flat_map(|level| {
                let at = match level {
                    Level::Own => "",
                    Level::Account => "account.",
                    Level::Source => "source.",
                };
                let contract = self.get(level);
                let per_library = contract
                    .keys
                    .iter()
                    .filter(move |(_, k)| level != Level::Own && (k.per_library || k.restart))
                    .map(|(n, k)| {
                        let what = if k.per_library {
                            "per_library"
                        } else {
                            "restart"
                        };
                        format!("setting `{n}`: `{what}` is for a plugin's own settings")
                    });
                contract
                    .problems()
                    .into_iter()
                    .chain(per_library)
                    .map(move |e| format!("{at}{e}"))
            })
            .collect()
    }
}

/// The translation message of a key, with the attributes `label` and
/// `help`: `setting-<owner>-<key>`, `account-setting-…` or
/// `source-setting-…`, the owner (a plugin's or a connector's id) and the
/// key slugged as Fluent identifiers.
pub fn message(owner: &str, level: Level, key: &str) -> String {
    let slug = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    };
    format!("{}-{}-{}", level.prefix(), slug(owner), slug(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Contract, String> {
        let table: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
        serde_json::from_value(table).map_err(|e| e.to_string())
    }

    fn sample() -> Contract {
        Contract::new()
            .key(
                "quality",
                ConfigKey::new(KeyType::Int)
                    .default_value(json!(90))
                    .range(1.0, 100.0, 1.0)
                    .label("Quality")
                    .group("Previews"),
            )
            .key(
                "mode",
                ConfigKey::new(KeyType::Choice)
                    .choices(&["auto", "custom"])
                    .default_value(json!("auto")),
            )
            .key(
                "folder",
                ConfigKey::new(KeyType::Path)
                    .path(PathKind::Folder)
                    .visible_when("mode", json!("custom")),
            )
            .key("token", ConfigKey::new(KeyType::Secret).label("Token"))
            .key(
                "fast",
                ConfigKey::new(KeyType::Bool).default_value(json!(true)),
            )
    }

    #[test]
    fn values_resolve_over_defaults_and_the_ones_refused_say_why_without_the_value() {
        let c = sample();
        let given = json!({
            "quality": 500,
            "mode": "custom",
            "token": "hunter2-secret",
            "speed": 3,
        });
        let (values, refused) = c.resolve(given.as_object().unwrap());
        assert_eq!(values["quality"], 90, "refused: the default stays");
        assert_eq!(values["mode"], "custom");
        assert_eq!(values["token"], "hunter2-secret");
        assert_eq!(values["fast"], true);
        assert_eq!(
            refused,
            vec![
                "`quality`: 500 is above 100; left out".to_string(),
                "`speed`: not declared; left out".to_string(),
            ]
        );
        let (_, refused) = c.resolve(json!({ "token": 7 }).as_object().unwrap());
        assert_eq!(refused, vec!["`token`: a secret is a text; left out"]);
        let contracts = Contracts {
            account: Contract::new().key("x", ConfigKey::new(KeyType::Bool).restart()),
            ..Contracts::default()
        };
        assert_eq!(
            contracts.problems(),
            vec!["account.setting `x`: `restart` is for a plugin's own settings"]
        );
    }

    #[test]
    fn a_sound_contract_has_no_problems_and_its_schema_checks_values() {
        let c = sample();
        assert_eq!(c.problems(), Vec::<String>::new());
        assert_eq!(c.check(&json!({ "quality": 50, "mode": "custom" })), Ok(()));
        assert_eq!(c.check(&json!({})), Ok(()));
        for (bad, error) in [
            (json!({ "quality": 0 }), "settings.quality: 0 is below 1"),
            (
                json!({ "quality": 2.5 }),
                "settings.quality: expected integer, got number",
            ),
            (
                json!({ "mode": "fast" }),
                "settings.mode: expected one of \"auto\", \"custom\"",
            ),
            (
                json!({ "speed": 1 }),
                "settings: `speed` is not one of its settings",
            ),
            (
                json!({ "folder": 3 }),
                "settings.folder: expected string, got integer",
            ),
        ] {
            assert_eq!(c.check(&bad), Err(error.into()), "{bad}");
        }
        let schema = c.schema();
        assert_eq!(schema["properties"]["token"]["writeOnly"], json!(true));
        assert_eq!(schema["properties"]["quality"]["default"], json!(90));
        assert_eq!(
            schema["properties"]["folder"]["format"],
            json!("folder-path")
        );
    }

    #[test]
    fn defaults_leave_secrets_out_and_visibility_follows_values() {
        let c = sample();
        let defaults = c.defaults();
        assert_eq!(defaults.get("quality"), Some(&json!(90)));
        assert!(!defaults.contains_key("token"));
        assert_eq!(c.secrets().collect::<Vec<_>>(), ["token"]);
        assert!(!c.visible("folder", &Map::new()));
        let custom: Map<String, Json> =
            [("mode".to_owned(), json!("custom"))].into_iter().collect();
        assert!(c.visible("folder", &custom));
        assert!(c.visible("quality", &Map::new()));
        // Grouped in the order their first key comes: the ungrouped keys
        // sort first by name, `quality` heads its own group.
        let names: Vec<&str> = c.ordered().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["fast", "folder", "mode", "token", "quality"]);
    }

    #[test]
    fn a_bad_declaration_is_named() {
        for (text, problem) in [
            (
                r#"{ "q": { "type": "int", "default": "high" } }"#,
                "setting `q`: default \"high\": expected integer, got string",
            ),
            (
                r#"{ "q": { "type": "int", "default": 200, "max": 100 } }"#,
                "setting `q`: default 200: 200 is above 100",
            ),
            (
                r#"{ "q": { "type": "choice", "default": "a" } }"#,
                "setting `q`: a choice without `choices`",
            ),
            (
                r#"{ "q": { "type": "secret", "default": "hunter2" } }"#,
                "setting `q`: a secret has no default: it would be written in the clear",
            ),
            (
                r#"{ "q": { "type": "path" } }"#,
                "setting `q`: a path without `path = \"file\"` or `\"folder\"`",
            ),
            (
                r#"{ "q": { "type": "string", "visible_when": { "nope": true } } }"#,
                "setting `q`: `visible_when` names `nope`, which it does not declare",
            ),
            (
                r#"{ "q": { "type": "float", "min": 2, "max": 1 } }"#,
                "setting `q`: `min` 2 is above `max` 1",
            ),
            (
                r#"{ "q": { "type": "secret", "per_library": true } }"#,
                "setting `q`: a secret is kept in the keyring, not per library",
            ),
            (
                r#"{ "Q": { "type": "bool" } }"#,
                "setting `Q`: a name of lowercase letters, digits, `-` and `_`",
            ),
        ] {
            let c = parse(text).unwrap();
            assert!(
                c.problems().contains(&problem.to_owned()),
                "{text}: {:?}",
                c.problems()
            );
        }
        let unknown = parse(r#"{ "q": { "type": "colour" } }"#).unwrap_err();
        assert!(unknown.contains("unknown variant `colour`"), "{unknown}");
        let typo = parse(r#"{ "q": { "type": "bool", "title": "Q" } }"#).unwrap_err();
        assert!(typo.contains("unknown field `title`"), "{typo}");
    }

    #[test]
    fn about_is_read_as_help_and_messages_are_slugged() {
        let c = parse(r#"{ "exe": { "type": "string", "default": "x", "about": "The program" } }"#)
            .unwrap();
        assert_eq!(c.get("exe").unwrap().help, "The program");
        assert_eq!(
            message("org.example.exif", Level::Account, "api_key"),
            "account-setting-org-example-exif-api-key"
        );
        assert_eq!(
            message("folder", Level::Source, "copy"),
            "source-setting-folder-copy"
        );
    }
}
