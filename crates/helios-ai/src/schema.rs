//! A JSON Schema subset, enough for prompt inputs and outputs: `type`
//! (one or a list), `properties`, `required`, `additionalProperties`
//! (false, or a schema), `items`, `enum`, `minimum`, `maximum`,
//! `minLength`, `maxLength`, `minItems`, `maxItems`. Other keywords are
//! ignored. Errors name the path of the value (`$.keywords[2]`).

use serde_json::Value;

/// Every way `value` does not fit `schema`; empty when it fits.
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(schema, value, "$", &mut errors);
    errors
}

/// Whether a schema uses only what [`validate`] understands, and its
/// types are known: the problems found.
pub fn lint(schema: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    lint_at(schema, "$", &mut problems);
    problems
}

const TYPES: [&str; 7] = [
    "object", "array", "string", "number", "integer", "boolean", "null",
];

fn lint_at(schema: &Value, path: &str, problems: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        problems.push(format!("{path}: a schema is an object"));
        return;
    };
    let types: Vec<&str> = match obj.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    for t in types {
        if !TYPES.contains(&t) {
            problems.push(format!("{path}: unknown type `{t}`"));
        }
    }
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (name, sub) in props {
            lint_at(sub, &format!("{path}.{name}"), problems);
        }
    }
    if let Some(items) = obj.get("items") {
        lint_at(items, &format!("{path}[]"), problems);
    }
}

fn type_of(value: &Value) -> &'static str {
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

fn fits_type(wanted: &str, value: &Value) -> bool {
    let actual = type_of(value);
    wanted == actual
        || (wanted == "number" && actual == "integer")
        || (wanted == "integer" && value.as_f64().is_some_and(|f| f.fract() == 0.0))
}

fn check(schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    let types: Vec<&str> = match obj.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !types.is_empty() && !types.iter().any(|t| fits_type(t, value)) {
        errors.push(format!(
            "{path}: expected {}, got {}",
            types.join(" or "),
            type_of(value)
        ));
        return;
    }
    if let Some(options) = obj.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        errors.push(format!(
            "{path}: {value} is not one of {}",
            Value::Array(options.clone())
        ));
    }
    if let Some(x) = value.as_f64() {
        if let Some(min) = obj.get("minimum").and_then(Value::as_f64)
            && x < min
        {
            errors.push(format!("{path}: {x} is below {min}"));
        }
        if let Some(max) = obj.get("maximum").and_then(Value::as_f64)
            && x > max
        {
            errors.push(format!("{path}: {x} is above {max}"));
        }
    }
    if let Some(s) = value.as_str() {
        let n = s.chars().count() as u64;
        if let Some(min) = obj.get("minLength").and_then(Value::as_u64)
            && n < min
        {
            errors.push(format!("{path}: shorter than {min} characters"));
        }
        if let Some(max) = obj.get("maxLength").and_then(Value::as_u64)
            && n > max
        {
            errors.push(format!("{path}: longer than {max} characters"));
        }
    }
    if let Some(items) = value.as_array() {
        let n = items.len() as u64;
        if let Some(min) = obj.get("minItems").and_then(Value::as_u64)
            && n < min
        {
            errors.push(format!("{path}: fewer than {min} items"));
        }
        if let Some(max) = obj.get("maxItems").and_then(Value::as_u64)
            && n > max
        {
            errors.push(format!("{path}: more than {max} items"));
        }
        if let Some(item) = obj.get("items") {
            for (i, v) in items.iter().enumerate() {
                check(item, v, &format!("{path}[{i}]"), errors);
            }
        }
    }
    if let Some(fields) = value.as_object() {
        let props = obj.get("properties").and_then(Value::as_object);
        for name in obj
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !fields.contains_key(name) {
                errors.push(format!("{path}: `{name}` is missing"));
            }
        }
        for (name, v) in fields {
            let sub_path = format!("{path}.{name}");
            match (
                props.and_then(|p| p.get(name)),
                obj.get("additionalProperties"),
            ) {
                (Some(sub), _) => check(sub, v, &sub_path, errors),
                (None, Some(Value::Bool(false))) => {
                    errors.push(format!("{path}: `{name}` is not expected"));
                }
                (None, Some(extra @ Value::Object(_))) => check(extra, v, &sub_path, errors),
                (None, _) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn caption() -> Value {
        json!({
            "type": "object",
            "required": ["caption", "keywords"],
            "additionalProperties": false,
            "properties": {
                "caption": { "type": "string", "minLength": 1, "maxLength": 20 },
                "keywords": { "type": "array", "items": { "type": "string" }, "maxItems": 2 },
                "score": { "type": "integer", "minimum": 1, "maximum": 5 },
                "mood": { "enum": ["calm", "busy"] }
            }
        })
    }

    #[test]
    fn a_fitting_value_has_no_error() {
        let ok = json!({ "caption": "a dog", "keywords": ["dog"], "score": 3, "mood": "calm" });
        assert!(validate(&caption(), &ok).is_empty());
    }

    #[test]
    fn every_misfit_is_named_by_path() {
        let bad = json!({
            "keywords": ["a", 2, "c"], "score": 9, "mood": "loud", "extra": true
        });
        let errors = validate(&caption(), &bad);
        let all = errors.join("\n");
        assert!(all.contains("`caption` is missing"), "{all}");
        assert!(all.contains("$.keywords: more than 2 items"), "{all}");
        assert!(
            all.contains("$.keywords[1]: expected string, got integer"),
            "{all}"
        );
        assert!(all.contains("$.score: 9 is above 5"), "{all}");
        assert!(all.contains("$.mood: \"loud\" is not one of"), "{all}");
        assert!(all.contains("`extra` is not expected"), "{all}");
    }

    #[test]
    fn integers_are_numbers_and_whole_floats_integers() {
        assert!(validate(&json!({ "type": "number" }), &json!(3)).is_empty());
        assert!(validate(&json!({ "type": "integer" }), &json!(3.0)).is_empty());
        assert!(!validate(&json!({ "type": "integer" }), &json!(3.5)).is_empty());
        assert!(validate(&json!({ "type": ["string", "null"] }), &json!(null)).is_empty());
    }

    #[test]
    fn unknown_types_are_linted() {
        assert!(lint(&caption()).is_empty());
        let problems =
            lint(&json!({ "type": "object", "properties": { "a": { "type": "text" } } }));
        assert_eq!(problems, vec!["$.a: unknown type `text`"]);
    }
}
