//! Named, typed parameter values.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Value {
    Float(f64),
    Int(i64),
    Bool(bool),
    Choice(String),
    /// Numbers that belong together (a color matrix, white balance
    /// coefficients); used by context fields, not by parameters yet.
    List(Vec<f64>),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            Value::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[f64]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_choice(&self) -> Option<&str> {
        match self {
            Value::Choice(v) => Some(v),
            _ => None,
        }
    }
}

/// A set of named parameter values. Ordered by name, so that it serializes
/// (and hashes) the same way whatever the order values were set in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Params(BTreeMap<String, Value>);

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, name: &str, value: Value) -> Self {
        self.set(name, value);
        self
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.0.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// A float parameter. The framework validated the set against the
    /// manifest, so a declared float is always present.
    pub fn float(&self, name: &str) -> f64 {
        self.get(name)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("no float parameter `{name}`"))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Canonical bytes, used in content keys.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.0).expect("parameters always serialize")
    }
}
