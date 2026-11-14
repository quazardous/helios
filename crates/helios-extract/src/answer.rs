//! A model's answer read back: parsed leniently, checked against the
//! keys, turned into typed values.
//!
//! What does not fit the schema is an error, to send back once with
//! [`retry_note`]. Values outside a finite list are not errors: they are
//! dropped (or mapped to `other`) and counted, never silently.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::decl::{ExtractorDef, KeyDecl, KeyType, Values};
use crate::schema::max_len;

/// One typed value of a key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum Value {
    Text { value: String },
    Label { value: String, p: Option<f64> },
    Number { value: f64 },
    Integer { value: i64 },
    Bool { value: bool },
}

impl Value {
    /// The value as text, for labels and texts.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text { value } | Self::Label { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// What the validator changed in an answer instead of refusing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjust {
    /// Outside a finite list: dropped.
    Dropped,
    /// Outside a finite list that accepts `other`: kept as `other`.
    MappedToOther,
    /// Its confidence was below the extractor's floor: dropped.
    BelowFloor,
    /// Outside a semi-finite list: kept, a value not seen before.
    New,
}

/// One adjustment, with the key and the value as the model gave it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adjustment {
    pub key: String,
    pub value: String,
    pub what: Adjust,
}

/// An answer that fits: the values per key (empty: none; at most one
/// for a single key) and what was adjusted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub values: BTreeMap<String, Vec<Value>>,
    pub adjustments: Vec<Adjustment>,
}

impl Answer {
    /// How many adjustments of this kind.
    pub fn count(&self, what: Adjust) -> usize {
        self.adjustments.iter().filter(|a| a.what == what).count()
    }

    /// A key's values.
    pub fn get(&self, key: &str) -> &[Value] {
        self.values.get(key).map_or(&[], Vec::as_slice)
    }
}

/// The JSON in a model's reply: the whole text, or inside code fences,
/// or from its first `{` to its last `}`.
pub fn parse_reply(text: &str) -> Result<Json, String> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str(t) {
        return Ok(v);
    }
    let unfenced = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .and_then(|r| r.trim_end().strip_suffix("```"))
        .map(str::trim);
    if let Some(inner) = unfenced
        && let Ok(v) = serde_json::from_str(inner)
    {
        return Ok(v);
    }
    if let (Some(a), Some(b)) = (t.find('{'), t.rfind('}'))
        && a < b
        && let Ok(v) = serde_json::from_str(&t[a..=b])
    {
        return Ok(v);
    }
    Err("$: the reply holds no JSON object".to_owned())
}

/// A reply's text checked against a definition.
pub fn check_reply(def: &ExtractorDef, text: &str) -> Result<Answer, Vec<String>> {
    let json = parse_reply(text).map_err(|e| vec![e])?;
    validate(def, &json)
}

/// An answer checked against a definition's keys: typed values, or every
/// way it does not fit (paths as in helios-ai, `$.animal[1].p`).
pub fn validate(def: &ExtractorDef, answer: &Json) -> Result<Answer, Vec<String>> {
    let Some(obj) = answer.as_object() else {
        return Err(vec![format!(
            "$: expected an object, got {}",
            type_of(answer)
        )]);
    };
    let mut errors = Vec::new();
    let mut out = Answer::default();
    for name in obj.keys() {
        if def.key(name).is_none() {
            errors.push(format!("$: `{name}` is not expected"));
        }
    }
    for k in &def.keys {
        let path = format!("$.{}", k.name);
        let Some(v) = obj.get(&k.name) else {
            errors.push(format!("$: `{}` is missing", k.name));
            continue;
        };
        let mut values = Vec::new();
        if k.multi {
            match v {
                Json::Null => {}
                Json::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        let p = format!("{path}[{i}]");
                        if let Some(x) =
                            read_item(def, k, item, &p, &mut errors, &mut out.adjustments)
                            && !values.contains(&x)
                        {
                            values.push(x);
                        }
                    }
                }
                other => errors.push(format!("{path}: expected array, got {}", type_of(other))),
            }
        } else if !is_none(v)
            && let Some(x) = read_item(def, k, v, &path, &mut errors, &mut out.adjustments)
        {
            values.push(x);
        }
        out.values.insert(k.name.clone(), values);
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

/// The note appended to the prompt for the one corrective retry.
pub fn retry_note(errors: &[String]) -> String {
    let mut s = String::from("Your answer did not fit:\n");
    for e in errors {
        s.push_str("- ");
        s.push_str(e);
        s.push('\n');
    }
    s.push_str("Answer again with one JSON object that fits.");
    s
}

fn is_none(v: &Json) -> bool {
    v.is_null() || v.as_str() == Some("none")
}

fn read_item(
    def: &ExtractorDef,
    k: &KeyDecl,
    item: &Json,
    path: &str,
    errors: &mut Vec<String>,
    adjustments: &mut Vec<Adjustment>,
) -> Option<Value> {
    let (bare, p) = if k.confidence {
        let Some(o) = item.as_object() else {
            errors.push(format!(
                "{path}: expected an object {{value, p}}, got {}",
                type_of(item)
            ));
            return None;
        };
        let mut bad = false;
        for name in o.keys() {
            if name != "value" && name != "p" {
                errors.push(format!("{path}: `{name}` is not expected"));
                bad = true;
            }
        }
        let p = match o.get("p") {
            None => {
                errors.push(format!("{path}: `p` is missing"));
                return None;
            }
            Some(p) => match p.as_f64() {
                Some(x) if (0.0..=1.0).contains(&x) => x,
                Some(x) => {
                    errors.push(format!("{path}.p: {x} is outside 0 to 1"));
                    return None;
                }
                None => {
                    errors.push(format!("{path}.p: expected number, got {}", type_of(p)));
                    return None;
                }
            },
        };
        let Some(value) = o.get("value") else {
            errors.push(format!("{path}: `value` is missing"));
            return None;
        };
        if bad {
            return None;
        }
        (value, Some(p))
    } else {
        (item, None)
    };
    let path = if k.confidence {
        format!("{path}.value")
    } else {
        path.to_owned()
    };
    let mut value = read_bare(k, bare, &path, errors, adjustments)?;
    if let (
        Some(p),
        Value::Label {
            value: text,
            p: slot,
        },
    ) = (p, &mut value)
    {
        if p < def.confidence_floor {
            adjustments.push(Adjustment {
                key: k.name.clone(),
                value: text.clone(),
                what: Adjust::BelowFloor,
            });
            return None;
        }
        *slot = Some(p);
    }
    Some(value)
}

/// A bare value checked and typed (a label without its confidence yet),
/// or `None` when empty, dropped or wrong.
fn read_bare(
    k: &KeyDecl,
    v: &Json,
    path: &str,
    errors: &mut Vec<String>,
    adjustments: &mut Vec<Adjustment>,
) -> Option<Value> {
    match k.ty {
        KeyType::Text | KeyType::Label => {
            let Some(s) = v.as_str() else {
                errors.push(format!("{path}: expected string, got {}", type_of(v)));
                return None;
            };
            let s = s.trim();
            if s.is_empty() || s == "none" {
                return None;
            }
            let typed = |value: String| {
                if k.ty == KeyType::Label {
                    Value::Label { value, p: None }
                } else {
                    Value::Text { value }
                }
            };
            let mut adjust = |what| {
                adjustments.push(Adjustment {
                    key: k.name.clone(),
                    value: s.to_owned(),
                    what,
                });
            };
            let text = match &k.values {
                Values::Finite(list) => {
                    if let Some(found) = list.iter().find(|x| x.eq_ignore_ascii_case(s)) {
                        Some(found.clone())
                    } else if k.other && s.eq_ignore_ascii_case("other") {
                        Some("other".to_owned())
                    } else if k.other {
                        adjust(Adjust::MappedToOther);
                        Some("other".to_owned())
                    } else {
                        adjust(Adjust::Dropped);
                        None
                    }
                }
                Values::SemiFinite(list) => {
                    if let Some(found) = list.iter().find(|x| x.eq_ignore_ascii_case(s)) {
                        Some(found.clone())
                    } else if fits_len(k, s, path, errors) {
                        adjust(Adjust::New);
                        Some(s.to_owned())
                    } else {
                        None
                    }
                }
                Values::Open => fits_len(k, s, path, errors).then(|| s.to_owned()),
            };
            text.map(typed)
        }
        KeyType::Number | KeyType::Integer => {
            let Some(x) = v.as_f64() else {
                errors.push(format!(
                    "{path}: expected {}, got {}",
                    k.ty.name(),
                    type_of(v)
                ));
                return None;
            };
            if k.ty == KeyType::Integer && x.fract() != 0.0 {
                errors.push(format!("{path}: expected integer, got {x}"));
                return None;
            }
            if let Some(min) = k.min
                && x < min
            {
                errors.push(format!("{path}: {x} is below {min}"));
                return None;
            }
            if let Some(max) = k.max
                && x > max
            {
                errors.push(format!("{path}: {x} is above {max}"));
                return None;
            }
            Some(if k.ty == KeyType::Integer {
                Value::Integer { value: x as i64 }
            } else {
                Value::Number { value: x }
            })
        }
        KeyType::Bool => {
            if let Some(value) = v.as_bool() {
                Some(Value::Bool { value })
            } else {
                errors.push(format!("{path}: expected boolean, got {}", type_of(v)));
                None
            }
        }
    }
}

fn fits_len(k: &KeyDecl, s: &str, path: &str, errors: &mut Vec<String>) -> bool {
    let max = max_len(k.ty);
    if s.chars().count() as u64 > max {
        errors.push(format!("{path}: longer than {max} characters"));
        false
    } else {
        true
    }
}

fn type_of(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}
