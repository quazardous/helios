//! What the vault records about an image besides its files: declared
//! fields, the commands that change them, their audit trail, and queries.

use serde::Serialize;
use serde_json::Value;

use crate::ImageId;

/// What a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A whole number within bounds.
    Int {
        min: i64,
        max: i64,
    },
    /// A set of names taken from a fixed list.
    Labels(&'static [&'static str]),
    Text,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: FieldKind,
    pub about: &'static str,
}

pub const RATING: &str = "rating";
pub const LABELS: &str = "labels";

/// The fields of an image, declared once.
pub const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: RATING,
        kind: FieldKind::Int { min: -1, max: 5 },
        about: "0 to 5 stars; -1 means rejected (as in darktable)",
    },
    FieldSpec {
        name: LABELS,
        kind: FieldKind::Labels(&["red", "yellow", "green", "blue", "purple"]),
        about: "color labels",
    },
    FieldSpec {
        name: "title",
        kind: FieldKind::Text,
        about: "title",
    },
    FieldSpec {
        name: "description",
        kind: FieldKind::Text,
        about: "description",
    },
];

pub fn spec(name: &str) -> Result<&'static FieldSpec, String> {
    FIELDS.iter().find(|f| f.name == name).ok_or_else(|| {
        let known: Vec<_> = FIELDS.iter().map(|f| f.name).collect();
        format!("unknown field `{name}` (known: {})", known.join(", "))
    })
}

/// Checks a value for a field and puts it in its canonical form (labels
/// sorted and unique), so that equal values compare equal.
pub fn normalize(name: &str, value: &Value) -> Result<Value, String> {
    let spec = spec(name)?;
    match (spec.kind, value) {
        (FieldKind::Int { min, max }, Value::Number(n)) => match n.as_i64() {
            Some(v) if (min..=max).contains(&v) => Ok(Value::from(v)),
            _ => Err(format!(
                "`{name}`: {n} is not a whole number in [{min}, {max}]"
            )),
        },
        (FieldKind::Labels(allowed), Value::Array(items)) => {
            let mut labels = Vec::new();
            for item in items {
                let label = item
                    .as_str()
                    .filter(|l| allowed.contains(l))
                    .ok_or_else(|| {
                        format!("`{name}`: {item} is not one of {}", allowed.join(", "))
                    })?;
                labels.push(label.to_string());
            }
            labels.sort_unstable();
            labels.dedup();
            Ok(Value::from(labels))
        }
        (FieldKind::Text, Value::String(s)) => Ok(Value::from(s.clone())),
        (_, other) => Err(format!("`{name}`: {other} has the wrong type")),
    }
}

/// A change to one field of one image; `None` clears it.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub image: ImageId,
    pub field: String,
    pub value: Option<Value>,
}

/// A recorded change: who changed what, from what, to what, from which job.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AuditEntry {
    /// Assigned by the store, increasing.
    pub seq: u64,
    pub at: u64,
    pub who: String,
    pub job: Option<u64>,
    pub image: ImageId,
    pub field: String,
    pub old: Option<Value>,
    pub new: Option<Value>,
}

/// How a query orders its images.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Id,
    /// Highest rating first, then by id.
    RatingDesc,
}

/// Which images a cursor holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// At least this rating (an image without a rating counts as 0).
    pub min_rating: Option<i64>,
    /// Carrying this color label.
    pub label: Option<String>,
    pub order: Order,
}
