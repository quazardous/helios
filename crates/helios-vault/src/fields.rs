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
    /// True or false.
    Bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: FieldKind,
    pub about: &'static str,
}

pub const RATING: &str = "rating";
pub const REJECTED: &str = "rejected";
pub const LABELS: &str = "labels";

/// The fields of an image, declared once.
pub const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: RATING,
        kind: FieldKind::Int { min: 0, max: 5 },
        about: "0 to 5 stars",
    },
    // A field of its own, not a rating of -1 as in darktable: taking a
    // reject back must not erase the stars the image had.
    FieldSpec {
        name: REJECTED,
        kind: FieldKind::Bool,
        about: "rejected; the stars are kept beside it",
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
        (FieldKind::Bool, Value::Bool(b)) => Ok(Value::from(*b)),
        (_, other) => Err(format!("`{name}`: {other} has the wrong type")),
    }
}

/// What a command does to its field.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Sets the field to this value, or with `None` clears it.
    Set(Option<Value>),
    /// Sets the field only when it holds nothing: a default, such as a
    /// sidecar's rating, never overwrites what a user chose.
    SetIfAbsent(Value),
    /// Adds a name to a set field (a color label), keeping the others.
    Add(String),
    /// Takes a name out of a set field, keeping the others.
    Remove(String),
}

/// A change to one field of one image. Every command says what it changes,
/// never "here is my copy": adding a label is applied by the vault to the
/// stored set, so a label another writer added meanwhile is kept
/// (ROADMAP 2.8, rule 1).
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub image: ImageId,
    pub field: String,
    pub op: Op,
}

impl Command {
    /// Sets a field to `value`.
    pub fn set(image: ImageId, field: &str, value: Value) -> Self {
        Self::with(image, field, Op::Set(Some(value)))
    }

    /// Clears a field.
    pub fn clear(image: ImageId, field: &str) -> Self {
        Self::with(image, field, Op::Set(None))
    }

    /// Adds `item` to a set field.
    pub fn add(image: ImageId, field: &str, item: &str) -> Self {
        Self::with(image, field, Op::Add(item.into()))
    }

    /// Takes `item` out of a set field.
    pub fn remove(image: ImageId, field: &str, item: &str) -> Self {
        Self::with(image, field, Op::Remove(item.into()))
    }

    pub fn with(image: ImageId, field: &str, op: Op) -> Self {
        Self {
            image,
            field: field.into(),
            op,
        }
    }

    /// Checks the command against its field's declaration, values in their
    /// canonical form.
    pub(crate) fn checked(&self) -> Result<Op, String> {
        let spec = spec(&self.field)?;
        match &self.op {
            Op::Set(None) => Ok(Op::Set(None)),
            Op::Set(Some(v)) => Ok(Op::Set(Some(normalize(&self.field, v)?))),
            Op::SetIfAbsent(v) => Ok(Op::SetIfAbsent(normalize(&self.field, v)?)),
            Op::Add(item) | Op::Remove(item) => {
                let FieldKind::Labels(allowed) = spec.kind else {
                    return Err(format!(
                        "`{}` is not a set: it takes no add or remove",
                        self.field
                    ));
                };
                if !allowed.contains(&item.as_str()) {
                    return Err(format!(
                        "`{}`: `{item}` is not one of {}",
                        self.field,
                        allowed.join(", ")
                    ));
                }
                Ok(self.op.clone())
            }
        }
    }
}

/// What checked operations make of a stored value, in order. A set left
/// empty is cleared.
pub(crate) fn fold(
    field: &str,
    stored: Option<Value>,
    ops: &[Op],
) -> Result<Option<Value>, String> {
    let mut value = stored;
    for op in ops {
        value = match op {
            Op::Set(v) => v.clone(),
            Op::SetIfAbsent(v) => value.or_else(|| Some(v.clone())),
            Op::Add(item) | Op::Remove(item) => {
                let mut items: Vec<Value> = value
                    .as_ref()
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                items.retain(|v| v.as_str() != Some(item.as_str()));
                if matches!(op, Op::Add(_)) {
                    items.push(Value::from(item.clone()));
                }
                if items.is_empty() {
                    None
                } else {
                    Some(normalize(field, &Value::from(items))?)
                }
            }
        };
    }
    Ok(value)
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
