//! What the vault records about an image besides its files: declared
//! fields, the commands that change them, their audit trail, and queries.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::storage::VolumeId;
use crate::{ImageId, Presence};

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
    /// A set of free names (tags): any non-empty text, kept trimmed.
    Tags,
    Text,
    /// True or false.
    Bool,
    /// A finite number, a fraction allowed (an aperture, a shutter time).
    Number,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub name: &'static str,
    pub kind: FieldKind,
    pub about: &'static str,
    /// Read from the file (its capture metadata, [`crate::capture`]): no
    /// command changes it, only [`crate::Vault::set_captures`].
    pub read_only: bool,
}

pub const RATING: &str = "rating";
pub const REJECTED: &str = "rejected";
pub const LABELS: &str = "labels";
/// Picked while culling; a field of its own beside `rejected`, so a pick
/// and a reject never share a rating's value.
pub const PICKED: &str = "picked";
pub const TAGS: &str = "tags";

/// The fields of an image, declared once.
pub const FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: RATING,
        kind: FieldKind::Int { min: 0, max: 5 },
        about: "0 to 5 stars",
        read_only: false,
    },
    // A field of its own, not a rating of -1 as in darktable: taking a
    // reject back must not erase the stars the image had.
    FieldSpec {
        name: REJECTED,
        kind: FieldKind::Bool,
        about: "rejected; the stars are kept beside it",
        read_only: false,
    },
    FieldSpec {
        name: LABELS,
        kind: FieldKind::Labels(&["red", "yellow", "green", "blue", "purple"]),
        about: "color labels",
        read_only: false,
    },
    FieldSpec {
        name: PICKED,
        kind: FieldKind::Bool,
        about: "picked while culling",
        read_only: false,
    },
    FieldSpec {
        name: TAGS,
        kind: FieldKind::Tags,
        about: "tags (keywords)",
        read_only: false,
    },
    FieldSpec {
        name: "title",
        kind: FieldKind::Text,
        about: "title",
        read_only: false,
    },
    FieldSpec {
        name: "description",
        kind: FieldKind::Text,
        about: "description",
        read_only: false,
    },
    // What the file says of how it was taken (`docs/design/library.md`,
    // L2): read-only, indexed like every field (`(name, number)`).
    FieldSpec {
        name: crate::capture::CAPTURED,
        kind: FieldKind::Number,
        about: "when it was taken: the camera's clock read as UTC, in seconds (the instant is this less `capture_offset`)",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::CAPTURE_OFFSET,
        kind: FieldKind::Int {
            min: -1440,
            max: 1440,
        },
        about: "the camera clock's offset from UTC, in minutes, when the file says it",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::CAPTURED_FROM,
        kind: FieldKind::Text,
        about: "where the capture time came from: `exif`, or `file` (its modification time, in UTC)",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::CAMERA_MAKE,
        kind: FieldKind::Text,
        about: "the camera's maker",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::CAMERA_MODEL,
        kind: FieldKind::Text,
        about: "the camera's model",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::LENS,
        kind: FieldKind::Text,
        about: "the lens",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::FOCAL_LENGTH,
        kind: FieldKind::Number,
        about: "the focal length, in millimetres",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::APERTURE,
        kind: FieldKind::Number,
        about: "the aperture, as an f-number",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::SHUTTER,
        kind: FieldKind::Number,
        about: "the exposure time, in seconds",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::ISO,
        kind: FieldKind::Int {
            min: 0,
            max: 100_000_000,
        },
        about: "the ISO speed",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::WIDTH,
        kind: FieldKind::Int {
            min: 1,
            max: 1_000_000,
        },
        about: "the width in pixels, as stored (before the orientation turns it)",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::HEIGHT,
        kind: FieldKind::Int {
            min: 1,
            max: 1_000_000,
        },
        about: "the height in pixels, as stored",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::ORIENTATION,
        kind: FieldKind::Int { min: 1, max: 8 },
        about: "the EXIF orientation, 1 to 8",
        read_only: true,
    },
    FieldSpec {
        name: crate::capture::FILE_TYPE,
        kind: FieldKind::Text,
        about: "the file's type, its extension in lower case (`cr3`, `jpg`)",
        read_only: true,
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
        (FieldKind::Tags, Value::Array(items)) => {
            let mut tags = Vec::new();
            for item in items {
                let tag = item
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| format!("`{name}`: {item} is not a tag"))?;
                tags.push(tag.to_string());
            }
            tags.sort_unstable();
            tags.dedup();
            Ok(Value::from(tags))
        }
        (FieldKind::Text, Value::String(s)) => Ok(Value::from(s.clone())),
        (FieldKind::Bool, Value::Bool(b)) => Ok(Value::from(*b)),
        (FieldKind::Number, Value::Number(n)) => match n.as_f64() {
            Some(v) if v.is_finite() => Ok(Value::Number(n.clone())),
            _ => Err(format!("`{name}`: {n} is not a finite number")),
        },
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
        if spec.read_only {
            return Err(format!(
                "`{}` is read from the file: no command changes it",
                self.field
            ));
        }
        match &self.op {
            Op::Set(None) => Ok(Op::Set(None)),
            Op::Set(Some(v)) => Ok(Op::Set(Some(normalize(&self.field, v)?))),
            Op::SetIfAbsent(v) => Ok(Op::SetIfAbsent(normalize(&self.field, v)?)),
            Op::Add(item) | Op::Remove(item) => match spec.kind {
                FieldKind::Labels(allowed) => {
                    if !allowed.contains(&item.as_str()) {
                        return Err(format!(
                            "`{}`: `{item}` is not one of {}",
                            self.field,
                            allowed.join(", ")
                        ));
                    }
                    Ok(self.op.clone())
                }
                FieldKind::Tags => {
                    let tag = item.trim();
                    if tag.is_empty() {
                        return Err(format!("`{}`: an empty tag", self.field));
                    }
                    Ok(match &self.op {
                        Op::Add(_) => Op::Add(tag.into()),
                        _ => Op::Remove(tag.into()),
                    })
                }
                _ => Err(format!(
                    "`{}` is not a set: it takes no add or remove",
                    self.field
                )),
            },
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

/// What a query sorts its images by (`docs/design/library.md`, L3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    /// The image's id (its content hash): no meaning, but stable.
    #[default]
    Id,
    /// When the photo was taken (the `captured` field); photos taken in
    /// the same second by their file name.
    Captured,
    /// When the photo entered the Library (its flag's time).
    Imported,
    /// Its file's name: the first copy present, by path, else the first
    /// copy known.
    Name,
    /// Its stars (an unrated photo counts as 0).
    Rating,
    /// When it was last edited (its newest version).
    Edited,
    /// Its file's size.
    Size,
}

impl Sort {
    /// Every sort, in the order a menu lists them.
    pub const ALL: [Sort; 7] = [
        Sort::Captured,
        Sort::Imported,
        Sort::Name,
        Sort::Rating,
        Sort::Edited,
        Sort::Size,
        Sort::Id,
    ];

    /// Its name, as commands and templates write it.
    pub fn name(self) -> &'static str {
        match self {
            Sort::Id => "id",
            Sort::Captured => "captured",
            Sort::Imported => "imported",
            Sort::Name => "name",
            Sort::Rating => "rating",
            Sort::Edited => "edited",
            Sort::Size => "size",
        }
    }

    /// The sort of that name.
    pub fn parse(name: &str) -> Option<Sort> {
        Sort::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// How a query orders its images: a sort and its direction. Whatever the
/// direction, the photos that have no value for the sort (never taken by
/// a camera, never edited, no file name) come last, and ties are broken
/// the same way every time (by file name for the capture time, then by
/// id, ascending), so the order is stable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Order {
    pub by: Sort,
    pub descending: bool,
}

impl Order {
    /// By id: what a query sorts by when told nothing.
    pub const ID: Order = Order::asc(Sort::Id);

    /// Smallest, oldest or first in the alphabet first.
    pub const fn asc(by: Sort) -> Order {
        Order {
            by,
            descending: false,
        }
    }

    /// Largest, newest or last in the alphabet first.
    pub const fn desc(by: Sort) -> Order {
        Order {
            by,
            descending: true,
        }
    }
}

/// Which images a cursor holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// At least this rating (an image without a rating counts as 0).
    pub min_rating: Option<i64>,
    /// Carrying this color label.
    pub label: Option<String>,
    /// Carrying one of these color labels (none given: any image).
    pub labels: Vec<String>,
    /// Flagged one of these ways: `pick`, `reject`, `none` (neither
    /// picked nor rejected); none given: any image.
    pub flags: Vec<String>,
    /// Carrying every one of these tags.
    pub tags: Vec<String>,
    /// Taken at or after this capture time (the `captured` field: the
    /// camera's clock read as UTC, in seconds); an image without one
    /// never matches a capture range.
    pub captured_from: Option<i64>,
    /// Taken before this capture time.
    pub captured_before: Option<i64>,
    /// Words each found, whatever their case, in the photo's name (its
    /// first present copy's file name, else the start of its id, as the
    /// Library names it) or in one of its tags.
    pub words: Vec<String>,
    /// Taken with one of these cameras (the `camera_model` field, exact).
    pub cameras: Vec<String>,
    /// Taken with one of these lenses (the `lens` field, exact).
    pub lenses: Vec<String>,
    /// Of one of these file types (the `file_type` field: `cr3`, `jpg`).
    pub file_types: Vec<String>,
    /// With a copy in this folder or below (an absolute path, present or
    /// not).
    pub folder: Option<String>,
    /// Present, offline or missing as one of these ([`Presence::of`]).
    pub presence: Vec<Presence>,
    /// Backed up in one of these states against a strategy.
    pub backup: Option<BackupFilter>,
    pub order: Order,
}

/// A photo's backups against the library's strategy, as the Library marks
/// them (`helios_storage::strategy::status` decides from the same facts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupState {
    /// Its verified copies meet the strategy.
    Met,
    /// Some verified copies, fewer than asked (or none off site when one
    /// must be).
    Short,
    /// Copies on backup volumes not verified by hash yet.
    Unverified,
    /// No backup copy at all.
    None,
}

impl BackupState {
    pub const ALL: [BackupState; 4] = [
        BackupState::Met,
        BackupState::Short,
        BackupState::Unverified,
        BackupState::None,
    ];

    /// Its name, as filters and facet counts write it.
    pub fn name(self) -> &'static str {
        match self {
            BackupState::Met => "met",
            BackupState::Short => "short",
            BackupState::Unverified => "unverified",
            BackupState::None => "none",
        }
    }

    /// The state from what the copies give: `have` verified drives,
    /// `unverified` drives with copies not verified (none of them among
    /// the verified ones), and whether a verified copy is off site.
    pub fn of(have: u64, unverified: u64, off_site: bool, rule: &Backups) -> BackupState {
        if have == 0 && unverified == 0 {
            BackupState::None
        } else if have >= u64::from(rule.copies) && (off_site || !rule.off_site_required) {
            BackupState::Met
        } else if unverified > 0 {
            BackupState::Unverified
        } else {
            BackupState::Short
        }
    }
}

/// A volume whose copies count as backups (a backup or archive role,
/// strongly identified), with the drive it is on: two volumes of one
/// drive are one copy.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BackupVolume {
    pub id: VolumeId,
    pub drive: String,
    pub off_site: bool,
}

/// What a backup state is measured against: the volumes that count and
/// the strategy's demands (`helios_storage::strategy::rule` makes it). A
/// copy counts when its location is not missing and is on one of
/// `volumes`; it is verified when the image's document in
/// [`crate::storage::VERIFIED`] holds `<volume>\t<path inside it>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Backups {
    pub volumes: Vec<BackupVolume>,
    /// Verified copies on distinct drives a photo needs (0: no strategy).
    pub copies: u8,
    pub off_site_required: bool,
}

/// A backup filter: the states taken (none: no photo), measured against
/// `rule`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BackupFilter {
    pub rule: Backups,
    pub states: Vec<BackupState>,
}

/// The facets a library counts (`docs/design/library.md`, L4), by name.
pub const FACETS: [&str; 8] = [
    "rating",
    "label",
    "flag",
    "camera",
    "lens",
    "file_type",
    "presence",
    "backup",
];

/// How many photos of a query carry each value of each facet ([`FACETS`]):
/// `rating` (0 to 5, unrated as 0), `label` (each colour; a photo with two
/// counts in both), `flag` (`pick`, `reject`, `none`), `camera`, `lens`,
/// `file_type` (photos without the value are not counted), `presence`
/// (`present`, `offline`, `missing`) and `backup` (each [`BackupState`],
/// only when a rule is given). A value no photo has is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Facets(pub BTreeMap<String, BTreeMap<String, u64>>);

impl Facets {
    /// The count of one value of a facet (0 when none).
    pub fn count(&self, facet: &str, value: &str) -> u64 {
        self.0
            .get(facet)
            .and_then(|v| v.get(value))
            .copied()
            .unwrap_or(0)
    }

    /// A facet's values and counts, by value.
    pub fn of(&self, facet: &str) -> Vec<(&str, u64)> {
        self.0
            .get(facet)
            .map(|v| v.iter().map(|(k, n)| (k.as_str(), *n)).collect())
            .unwrap_or_default()
    }

    /// Adds one photo to a value.
    pub fn add(&mut self, facet: &str, value: &str, n: u64) {
        *self
            .0
            .entry(facet.to_string())
            .or_default()
            .entry(value.to_string())
            .or_default() += n;
    }
}

/// Folds a word or a name for the case-blind match of [`Query::words`]:
/// lower case, as Rust's `to_lowercase` gives it (the SQLite driver calls
/// the same function).
pub fn fold_case(text: &str) -> String {
    text.to_lowercase()
}

/// The flags a query takes.
pub const FLAGS: [&str; 3] = ["pick", "reject", "none"];

impl Query {
    /// Checks the flags named.
    pub fn check(&self) -> Result<(), String> {
        match self.flags.iter().find(|f| !FLAGS.contains(&f.as_str())) {
            Some(f) => Err(format!("`{f}` is not a flag ({})", FLAGS.join(", "))),
            None => Ok(()),
        }
    }

    /// An image's flag from its `picked` and `rejected` fields: a reject
    /// wins over a pick.
    pub fn flag_of(picked: bool, rejected: bool) -> &'static str {
        if rejected {
            "reject"
        } else if picked {
            "pick"
        } else {
            "none"
        }
    }
}
