//! The small part of JSON Schema Helios checks, shared by the node's
//! command params (`helios-node-core`) and the configuration contracts
//! ([`crate::config`]): `type` (one name or a list), `enum`, `properties`,
//! `required`, `additionalProperties` (a boolean or a schema), `items`,
//! `minItems`, `maxItems`, `minimum`, `maximum` and `anyOf`; `title`,
//! `description`, `default`, `format` and `writeOnly` are for the reader.

use serde_json::Value;

/// Checks `value` against `schema`; the first mismatch, named by its path
/// (`params.region.width`).
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, value, "params", "params")
}

/// [`validate`], the mismatch named from `root` (`settings.quality`), an
/// unknown key said not to be one of its `root`.
pub fn validate_at(schema: &Value, value: &Value, root: &str) -> Result<(), String> {
    check(schema, value, root, root)
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_type(value: &Value, name: &str) -> bool {
    match name {
        "number" => value.is_number(),
        "integer" => matches!(value, Value::Number(n) if n.is_i64() || n.is_u64()),
        other => type_name(value) == other,
    }
}

fn check(schema: &Value, value: &Value, path: &str, root: &str) -> Result<(), String> {
    if let Some(any) = schema.get("anyOf").and_then(Value::as_array) {
        let errors: Vec<String> = any
            .iter()
            .filter_map(|s| check(s, value, path, root).err())
            .collect();
        if errors.len() == any.len() {
            return Err(match errors.as_slice() {
                [one] => one.clone(),
                _ => format!("{path}: matches none of its forms ({})", errors.join("; ")),
            });
        }
    }
    if let Some(types) = schema.get("type") {
        let names: Vec<&str> = match types {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !names.iter().any(|name| is_type(value, name)) {
            return Err(format!(
                "{path}: expected {}, got {}",
                names.join(" or "),
                type_name(value)
            ));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        let names: Vec<String> = allowed.iter().map(Value::to_string).collect();
        return Err(format!("{path}: expected one of {}", names.join(", ")));
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            return Err(format!("{path}: {n} is below {min}"));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            return Err(format!("{path}: {n} is above {max}"));
        }
    }
    if let Value::Array(items) = value {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            return Err(format!("{path}: needs at least {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            return Err(format!("{path}: takes at most {max} items"));
        }
        if let Some(item) = schema.get("items") {
            for (i, v) in items.iter().enumerate() {
                check(item, v, &format!("{path}[{i}]"), root)?;
            }
        }
    }
    if let Value::Object(fields) = value {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !fields.contains_key(name) {
                    return Err(format!("{path}: `{name}` is missing"));
                }
            }
        }
        for (name, v) in fields {
            let at = format!("{path}.{name}");
            match properties.and_then(|p| p.get(name)) {
                Some(property) => check(property, v, &at, root)?,
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        return Err(format!("{path}: `{name}` is not one of its {root}"));
                    }
                    Some(extra @ Value::Object(_)) => check(extra, v, &at, root)?,
                    _ => {}
                },
            }
        }
    }
    Ok(())
}
