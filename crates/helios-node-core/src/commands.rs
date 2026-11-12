//! The node's commands: every verb a node answers, named, with the JSON
//! schema of its params and what it needs to run. A client lists them
//! (`commands.list`) with whether each can run here and why not, and runs
//! one by its id (`command.run`), its params checked against its schema
//! first. The shape follows the desktop's command registry
//! (`helios-shell`), plus the params schema and the needs a node checks.
//!
//! The schemas use the small part of JSON Schema [`validate`] checks
//! (`helios_core::schema`, shared with the configuration contracts).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value, json};

pub use helios_core::schema::validate;

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

/// The commands a node knows, by id.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    commands: BTreeMap<String, CommandSpec>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a command; a second declaration of an id is refused.
    pub fn register(&mut self, spec: CommandSpec) -> Result<(), String> {
        if self.commands.contains_key(&spec.id) {
            return Err(format!("command `{}` is already declared", spec.id));
        }
        self.commands.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn spec(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.get(id)
    }

    /// Every command, by id.
    pub fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.commands.values()
    }
}

/// Who asked for a command run: `user`, `cli`, `control`, `agent:NAME` or
/// `script:NAME`. Refused with the forms it takes otherwise.
pub fn check_origin(origin: &str) -> Result<(), String> {
    let named = |prefix: &str| {
        origin
            .strip_prefix(prefix)
            .is_some_and(|name| !name.is_empty() && !name.contains(char::is_whitespace))
    };
    if matches!(origin, "user" | "cli" | "control") || named("agent:") || named("script:") {
        Ok(())
    } else {
        Err(format!(
            "`{origin}` is not an origin: user, cli, control, agent:NAME or script:NAME"
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        object(&[
            ("node", json!({ "type": "string" }), true),
            (
                "quality",
                json!({ "type": ["integer", "null"], "minimum": 1, "maximum": 100 }),
                false,
            ),
            (
                "corners",
                json!({ "type": "array", "minItems": 2, "maxItems": 2,
                        "items": { "type": "number" } }),
                false,
            ),
            (
                "format",
                json!({ "type": "string", "enum": ["jpeg", "png"] }),
                false,
            ),
            (
                "values",
                json!({ "type": "object", "additionalProperties": { "anyOf": [
                    object(&[("float", json!({ "type": "number" }), true)]),
                    object(&[("int", json!({ "type": "integer" }), true)]),
                ] } }),
                false,
            ),
        ])
    }

    #[test]
    fn a_value_that_fits_passes() {
        let ok = json!({
            "node": "exp",
            "quality": null,
            "corners": [1, 2.5],
            "format": "png",
            "values": { "a": { "float": 1.5 }, "b": { "int": 2 } },
        });
        assert_eq!(validate(&schema(), &ok), Ok(()));
    }

    #[test]
    fn each_mismatch_is_named_by_its_path() {
        for (value, error) in [
            (json!({}), "params: `node` is missing"),
            (
                json!({ "node": 3 }),
                "params.node: expected string, got integer",
            ),
            (
                json!({ "node": "a", "speed": 1 }),
                "params: `speed` is not one of its params",
            ),
            (
                json!({ "node": "a", "quality": 0 }),
                "params.quality: 0 is below 1",
            ),
            (
                json!({ "node": "a", "corners": [1] }),
                "params.corners: needs at least 2 items",
            ),
            (
                json!({ "node": "a", "corners": [1, "x"] }),
                "params.corners[1]: expected number, got string",
            ),
            (
                json!({ "node": "a", "format": "gif" }),
                "params.format: expected one of \"jpeg\", \"png\"",
            ),
        ] {
            assert_eq!(validate(&schema(), &value), Err(error.into()), "{value}");
        }
        let wrong = validate(
            &schema(),
            &json!({ "node": "a", "values": { "a": { "text": "x" } } }),
        )
        .unwrap_err();
        assert!(
            wrong.starts_with("params.values.a: matches none of its forms"),
            "{wrong}"
        );
    }

    #[test]
    fn an_origin_is_one_of_its_forms() {
        for good in ["user", "cli", "control", "agent:mcp", "script:batch"] {
            assert_eq!(check_origin(good), Ok(()), "{good}");
        }
        for bad in ["", "agent:", "robot", "agent:two words"] {
            assert!(check_origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_id_is_declared_once() {
        let mut registry = Registry::new();
        registry
            .register(CommandSpec::new("pause", "Pause", "Jobs"))
            .unwrap();
        assert!(
            registry
                .register(CommandSpec::new("pause", "Again", "Jobs"))
                .is_err()
        );
        assert!(
            CommandSpec::new("cache.clear", "Clear the caches", "Cache").matches("CLEAR cache")
        );
        assert!(!CommandSpec::new("pause", "Pause", "Jobs").matches("cache"));
    }
}
