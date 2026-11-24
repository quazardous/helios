//! JSON Schema pieces the verbs of several crates share.

use serde_json::{Value, json};

use crate::object;

pub fn string() -> Value {
    json!({ "type": "string" })
}

/// A string, with what it holds.
pub fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

pub fn absolute() -> Value {
    text("an absolute path")
}

pub fn boolean() -> Value {
    json!({ "type": "boolean" })
}

pub fn count() -> Value {
    json!({ "type": "integer", "minimum": 0 })
}

pub fn strings() -> Value {
    json!({ "type": "array", "items": { "type": "string" } })
}

/// The same schema, `null` also taken (an optional field).
pub fn or_null(mut schema: Value) -> Value {
    let named = match schema.get("type") {
        Some(Value::String(one)) => vec![json!(one)],
        Some(Value::Array(many)) => many.clone(),
        _ => return schema,
    };
    let mut types = named;
    types.push(json!("null"));
    schema["type"] = Value::Array(types);
    schema
}

/// A parameter value: one of its kinds, `{ "float": 1.0 }`.
pub fn value() -> Value {
    let one = |kind: &str, schema: Value| object(&[(kind, schema, true)]);
    json!({ "anyOf": [
        one("float", json!({ "type": "number" })),
        one("int", json!({ "type": "integer" })),
        one("bool", boolean()),
        one("choice", string()),
        one("list", json!({ "type": "array", "items": { "type": "number" } })),
        one("ints", json!({ "type": "array", "items": { "type": "integer" } })),
        one("text", string()),
    ] })
}

/// Parameters by name.
pub fn params() -> Value {
    json!({ "type": "object", "additionalProperties": value() })
}

/// Parameters by node name, then by name.
pub fn node_params() -> Value {
    json!({ "type": "object", "additionalProperties": params(),
            "description": "parameters changed for this request only, by node name" })
}

/// An object of one required string field.
pub fn named(field: &str) -> Value {
    object(&[(field, string(), true)])
}
