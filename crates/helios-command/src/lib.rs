//! Helios's node command contract: what a verb declares. Each crate
//! declares the verbs of its own concept as a `verbs()` set of
//! [`CommandSpec`]s (the workflow's, the decisions', the caches'…), and the
//! node gathers the sets into its registry (`helios_node_core::commands`),
//! which lists them (`commands.list`), checks their params and runs them.
//!
//! This crate depends on nothing of Helios, so the lowest crates (the cache
//! service) can declare their verbs too. [`schema`] holds the small JSON
//! Schema pieces verbs share.

use serde::Serialize;
use serde_json::{Map, Value, json};

pub mod schema;

/// What a command needs to run, beyond the node itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Need {
    /// The vault the node owns (`helios-node --vault`).
    Vault,
    /// A desktop window holding the workspace.
    Window,
}

/// What a command declares.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommandSpec {
    /// Stable, never translated: `export`, `workflow.submit`.
    pub id: String,
    pub label: String,
    pub category: String,
    /// The JSON schema of its params: an object.
    pub params: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<Need>,
    /// It only reads.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}

impl CommandSpec {
    /// A command taking no params.
    pub fn new(id: &str, label: &str, category: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            category: category.into(),
            params: object(&[]),
            needs: Vec::new(),
            read_only: false,
        }
    }

    pub fn params(mut self, schema: Value) -> Self {
        self.params = schema;
        self
    }

    pub fn needs(mut self, need: Need) -> Self {
        self.needs.push(need);
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Whether every word of `filter` is in its id, label or category.
    pub fn matches(&self, filter: &str) -> bool {
        let hay = format!("{} {} {}", self.id, self.label, self.category).to_lowercase();
        filter
            .split_whitespace()
            .all(|word| hay.contains(&word.to_lowercase()))
    }
}

/// An object schema: its properties, each `(name, schema, required)`; no
/// other property is taken.
pub fn object(properties: &[(&str, Value, bool)]) -> Value {
    let props: Map<String, Value> = properties
        .iter()
        .map(|(name, schema, _)| ((*name).to_string(), schema.clone()))
        .collect();
    let required: Vec<&str> = properties
        .iter()
        .filter(|(_, _, required)| *required)
        .map(|(name, _, _)| *name)
        .collect();
    let mut schema = json!({
        "type": "object",
        "properties": props,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}
