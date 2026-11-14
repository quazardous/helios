//! The answer schema derived from an extractor's keys, and the words
//! that describe each key to a model.
//!
//! The schema stays within helios-ai's JSON Schema subset (`type`,
//! `properties`, `required`, `additionalProperties`, `items`, `enum`,
//! bounds), so it goes to a provider as structured output and is checked
//! on return the same way as a prompt's output schema.

use std::fmt::Write as _;

use serde_json::{Map, Value as Json, json};

use crate::decl::{ExtractorDef, KeyDecl, KeyType, Values};

/// The longest free text a single text key takes.
pub const MAX_TEXT: u64 = 300;
/// The longest label.
pub const MAX_LABEL: u64 = 100;
/// The most values a multi-valued key takes.
pub const MAX_ITEMS: u64 = 30;

/// The JSON schema of a model's answer: one object, every key required,
/// no other property.
pub fn answer_schema(def: &ExtractorDef) -> Json {
    let mut props = Map::new();
    for k in &def.keys {
        props.insert(k.name.clone(), key_schema(k));
    }
    json!({
        "type": "object",
        "required": def.keys.iter().map(|k| k.name.clone()).collect::<Vec<_>>(),
        "additionalProperties": false,
        "properties": props,
    })
}

/// The schema of one key's answer.
///
/// | Declared | Schema |
/// |---|---|
/// | finite, single | `{"enum": [values…, "none"]}` |
/// | finite, multi | an array of the enum (empty: none) |
/// | with confidence | each value is `{"value": …, "p": 0..1}` |
/// | open or semi-finite | a non-empty string (semi-finite: `examples`) |
/// | number / integer / bool | that type, with its bounds |
///
/// A single key that is not finite may be `null` (nothing found).
pub fn key_schema(k: &KeyDecl) -> Json {
    let item = item_schema(k);
    if k.multi {
        json!({ "type": "array", "items": item, "maxItems": MAX_ITEMS })
    } else if matches!(k.values, Values::Finite(_)) {
        item
    } else {
        nullable(item)
    }
}

fn item_schema(k: &KeyDecl) -> Json {
    let value = value_schema(k);
    if k.confidence {
        json!({
            "type": "object",
            "required": ["value", "p"],
            "additionalProperties": false,
            "properties": {
                "value": value,
                "p": { "type": "number", "minimum": 0, "maximum": 1 },
            },
        })
    } else {
        value
    }
}

/// The schema of one bare value of a key.
fn value_schema(k: &KeyDecl) -> Json {
    let mut s = Map::new();
    match k.ty {
        KeyType::Text | KeyType::Label => {
            s.insert("type".into(), "string".into());
            match &k.values {
                Values::Finite(list) => {
                    let mut options: Vec<Json> = list.iter().map(|v| v.as_str().into()).collect();
                    if k.other {
                        options.push("other".into());
                    }
                    if !k.multi {
                        options.push("none".into());
                    }
                    s.insert("enum".into(), options.into());
                }
                Values::SemiFinite(list) => {
                    s.insert("minLength".into(), 1.into());
                    s.insert("maxLength".into(), max_len(k.ty).into());
                    s.insert("examples".into(), list.clone().into());
                }
                Values::Open => {
                    s.insert("minLength".into(), 1.into());
                    s.insert("maxLength".into(), max_len(k.ty).into());
                }
            }
        }
        KeyType::Number | KeyType::Integer => {
            let t = if k.ty == KeyType::Number {
                "number"
            } else {
                "integer"
            };
            s.insert("type".into(), t.into());
            if let Some(min) = k.min {
                s.insert("minimum".into(), min.into());
            }
            if let Some(max) = k.max {
                s.insert("maximum".into(), max.into());
            }
        }
        KeyType::Bool => {
            s.insert("type".into(), "boolean".into());
        }
    }
    Json::Object(s)
}

/// The longest string a key's value takes.
pub(crate) fn max_len(ty: KeyType) -> u64 {
    if ty == KeyType::Text {
        MAX_TEXT
    } else {
        MAX_LABEL
    }
}

fn nullable(mut schema: Json) -> Json {
    if let Some(obj) = schema.as_object_mut()
        && let Some(Json::String(t)) = obj.get("type").cloned()
    {
        obj.insert("type".into(), json!([t, "null"]));
    }
    schema
}

/// One sentence per key telling a model what to answer, appended to an
/// LLM extractor's prompt (the schema alone is not given to every
/// provider as structured output).
pub fn key_instructions(def: &ExtractorDef) -> String {
    let mut out = String::from("Answer with one JSON object and nothing else, with these keys:\n");
    for k in &def.keys {
        out.push_str("- ");
        out.push_str(&key_sentence(k));
        out.push('\n');
    }
    out
}

fn key_sentence(k: &KeyDecl) -> String {
    let what = match (&k.values, k.ty) {
        (Values::Finite(list), _) => {
            let mut s = format!("any of {}", list.join(", "));
            if k.other {
                s.push_str(", or other");
            }
            s
        }
        (Values::SemiFinite(list), _) => {
            format!("a short label such as {}, or another", list.join(", "))
        }
        (Values::Open, KeyType::Text) => "free text".to_owned(),
        (Values::Open, KeyType::Label) => "a short label".to_owned(),
        (Values::Open, KeyType::Bool) => "true or false".to_owned(),
        (Values::Open, t @ (KeyType::Number | KeyType::Integer)) => {
            let mut s = if t == KeyType::Number {
                "a number".to_owned()
            } else {
                "a whole number".to_owned()
            };
            match (k.min, k.max) {
                (Some(lo), Some(hi)) => {
                    let _ = write!(s, " from {lo} to {hi}");
                }
                (Some(lo), None) => {
                    let _ = write!(s, " of at least {lo}");
                }
                (None, Some(hi)) => {
                    let _ = write!(s, " of at most {hi}");
                }
                (None, None) => {}
            }
            if let Some(u) = &k.unit {
                let _ = write!(s, " ({u})");
            }
            s
        }
    };
    let how_many = if k.multi {
        "; several allowed, as a list (empty when none)"
    } else if matches!(k.values, Values::Finite(_)) {
        "; one only, or none"
    } else {
        "; one only, or null when unknown"
    };
    let confidence = if k.confidence {
        "; give each as {\"value\": ..., \"p\": confidence between 0 and 1}"
    } else {
        ""
    };
    format!("`{}`: {what}{how_many}{confidence}.", k.name)
}
