//! The JSON schemas of the files (`templates/schema/`), and a checker for
//! the part of JSON Schema they use, so a file (a template an AI wrote)
//! is checked against them as they are published.
//!
//! The checker knows `$ref` (to `#/$defs/…`), `type`, `properties`,
//! `required`, `additionalProperties`, `items`, `minItems`, `enum`,
//! `const`, `minimum`, `oneOf` and `anyOf`; other keywords (descriptions,
//! patterns) are left to the types, which refuse what the schemas refuse.

use serde_json::Value as Json;

/// `template.json`'s schema.
pub const TEMPLATE: &str = include_str!("../../../templates/schema/template.schema.json");

/// A composite file's schema.
pub const COMPOSITE: &str = include_str!("../../../templates/schema/composite.schema.json");

/// `template.json`'s schema, parsed.
pub fn template() -> Json {
    serde_json::from_str(TEMPLATE).expect("the template schema is JSON")
}

/// A composite file's schema, parsed.
pub fn composite() -> Json {
    serde_json::from_str(COMPOSITE).expect("the composite schema is JSON")
}

/// What `value` breaks of `schema`, each with where (`/network/nodes/1`);
/// empty when it fits.
pub fn check(schema: &Json, value: &Json) -> Vec<String> {
    let mut out = Vec::new();
    walk(schema, schema, value, "", &mut out);
    out
}

fn walk(root: &Json, schema: &Json, value: &Json, at: &str, out: &mut Vec<String>) {
    let here = || {
        if at.is_empty() {
            "/".to_string()
        } else {
            at.to_string()
        }
    };
    if let Some(r) = schema.get("$ref").and_then(Json::as_str) {
        match r
            .strip_prefix("#/$defs/")
            .and_then(|name| root["$defs"].get(name))
        {
            Some(def) => walk(root, def, value, at, out),
            None => out.push(format!("{}: unknown reference {r}", here())),
        }
        return;
    }
    if let Some(ty) = schema.get("type") {
        let types: Vec<&str> = match ty {
            Json::String(t) => vec![t.as_str()],
            Json::Array(ts) => ts.iter().filter_map(Json::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.iter().any(|t| is(value, t)) {
            out.push(format!("{}: expected {}", here(), types.join(" or ")));
            return;
        }
    }
    if let Some(options) = schema.get("enum").and_then(Json::as_array)
        && !options.contains(value)
    {
        out.push(format!(
            "{}: {value} is not one of {}",
            here(),
            Json::Array(options.clone())
        ));
    }
    if let Some(c) = schema.get("const")
        && c != value
    {
        out.push(format!("{}: expected {c}", here()));
    }
    if let (Some(min), Some(v)) = (schema.get("minimum").and_then(Json::as_f64), value.as_f64())
        && v < min
    {
        out.push(format!("{}: {v} is below {min}", here()));
    }
    for (key, exactly_one) in [("oneOf", true), ("anyOf", false)] {
        if let Some(options) = schema.get(key).and_then(Json::as_array) {
            let fits = options
                .iter()
                .filter(|o| check_at(root, o, value, at).is_empty())
                .count();
            if fits == 0 || (exactly_one && fits > 1) {
                out.push(format!("{}: fits {fits} of the {key} options", here()));
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Json::as_object);
        for name in schema
            .get("required")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            if !object.contains_key(name) {
                out.push(format!("{}: `{name}` is missing", here()));
            }
        }
        for (k, v) in object {
            let path = format!("{at}/{k}");
            match properties.and_then(|p| p.get(k)) {
                Some(s) => walk(root, s, v, &path, out),
                None => match schema.get("additionalProperties") {
                    Some(Json::Bool(false)) => {
                        out.push(format!("{}: unknown field `{k}`", here()));
                    }
                    Some(s @ Json::Object(_)) => walk(root, s, v, &path, out),
                    _ => {}
                },
            }
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Json::as_u64)
            && (items.len() as u64) < min
        {
            out.push(format!("{}: fewer than {min} items", here()));
        }
        if let Some(s) = schema.get("items") {
            for (i, v) in items.iter().enumerate() {
                walk(root, s, v, &format!("{at}/{i}"), out);
            }
        }
    }
}

fn check_at(root: &Json, schema: &Json, value: &Json, at: &str) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, schema, value, at, &mut out);
    out
}

fn is(value: &Json, ty: &str) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_schemas_parse_and_name_their_kind() {
        assert_eq!(template()["title"], json!("Helios workspace template"));
        assert_eq!(composite()["title"], json!("Helios composite"));
    }

    #[test]
    fn the_checker_reports_where() {
        let schema = json!({
            "type": "object",
            "required": ["a"],
            "additionalProperties": false,
            "properties": {
                "a": { "type": "integer", "minimum": 1 },
                "b": { "type": "array", "items": { "$ref": "#/$defs/v" } }
            },
            "$defs": { "v": { "enum": ["x", "y"] } }
        });
        assert!(check(&schema, &json!({ "a": 2, "b": ["x"] })).is_empty());
        let problems = check(&schema, &json!({ "a": 0, "b": ["z"], "c": 1 }));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("/a: 0 is below 1")));
        assert!(problems.iter().any(|p| p.starts_with("/b/0:")));
        assert!(problems.iter().any(|p| p.contains("unknown field `c`")));
        assert_eq!(check(&schema, &json!({})), ["/: `a` is missing"]);
    }
}
