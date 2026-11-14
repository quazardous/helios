//! What a saved import takes and what it gives the photos it brings in
//! (`docs/design/import-window.md`, "The choices every source shares"):
//!
//! - a [`Filter`]: which files of the source are imported (their kinds,
//!   names, days, sizes, and only those the Library has not seen at that
//!   place), as an import's `filter` argument;
//! - an [`Apply`]: the keywords, rating and colour label every photo
//!   imported is given, as Lightroom's "Apply During Import" metadata
//!   does, as an import's `apply` argument.
//!
//! Both are kept on a saved source with its other choices, read by
//! whichever runs the import (the desktop, the node, an automatic import)
//! through [`read`], and recorded in the import history with the choices.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::import::{Metadata, Options};

/// A kind of file a filter takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A camera raw (`.cr3`, `.nef`, `.dng`…).
    Raw,
    /// A JPEG.
    Jpeg,
    /// A video clip.
    Video,
    /// Any other image (PNG, TIFF, HEIC…).
    Other,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Raw, Kind::Jpeg, Kind::Video, Kind::Other];

    /// The kind of the file at `path`, by its extension.
    pub fn of(path: &Path) -> Kind {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "jpg" | "jpeg" => Kind::Jpeg,
            "mp4" | "mov" | "avi" | "mts" | "m2ts" | "mkv" | "3gp" => Kind::Video,
            "png" | "tif" | "tiff" | "exr" | "heic" | "avif" | "jxl" | "webp" | "" => Kind::Other,
            e if helios_vault::IMAGE_EXTENSIONS.contains(&e) => Kind::Raw,
            _ => Kind::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Raw => "raw",
            Kind::Jpeg => "jpeg",
            Kind::Video => "video",
            Kind::Other => "other",
        }
    }
}

/// Which files of a source an import takes. Every part left empty takes
/// all: the default filter takes every file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// The kinds taken; empty: all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<Kind>,
    /// Name patterns (`*` any run, `?` one character, case ignored) on
    /// the file's name, extension included (`IMG_*`, `*.cr3`); a file is
    /// taken when one matches. Empty: all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// The first day taken (`YYYY-MM-DD`), by the file's time (a camera
    /// writes it as it shoots).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The last day taken (`YYYY-MM-DD`), included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The smallest file taken, in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_bytes: Option<u64>,
    /// The largest file taken, in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    /// Only files the Library has not recorded at that place.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub new_only: bool,
}

impl Filter {
    /// Whether it takes every file.
    pub fn is_all(&self) -> bool {
        *self == Filter::default()
    }

    /// Whether it takes the file at `path`, of `size` bytes, last written
    /// at `modified` (seconds since the epoch). Whether the Library knows
    /// it ([`Filter::new_only`]) is the caller's to check.
    pub fn takes(&self, path: &Path, size: u64, modified: Option<u64>) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&Kind::of(path)) {
            return false;
        }
        if !self.names.is_empty() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !self
                .names
                .iter()
                .any(|p| glob(&p.trim().to_lowercase(), &name))
            {
                return false;
            }
        }
        if self.min_bytes.is_some_and(|m| size < m) || self.max_bytes.is_some_and(|m| size > m) {
            return false;
        }
        let (from, to) = (day(self.from.as_deref()), day(self.to.as_deref()));
        if from.is_some() || to.is_some() {
            let Some(at) = modified else {
                return false;
            };
            let (y, m, d) = crate::signals::civil((at / 86_400) as i64);
            let on = format!("{y:04}-{m:02}-{d:02}");
            if from.is_some_and(|f| on.as_str() < f) || to.is_some_and(|t| on.as_str() > t) {
                return false;
            }
        }
        true
    }

    /// Whether it takes the file at `path`, read from the disk.
    pub fn takes_file(&self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        self.takes(path, meta.len(), modified)
    }
}

/// A day given as `YYYY-MM-DD`, else none.
fn day(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|t| {
        t.len() == 10
            && t.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
    })
}

/// Whether `text` matches `pattern` (`*` any run, `?` one character).
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// The colour labels a photo can be given.
pub const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// What every photo an import brings in is given ("Apply During Import").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Apply {
    /// Keywords added.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// A rating, 0 to 5 stars, set where the photo has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    /// A colour label added ([`LABELS`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Apply {
    /// Whether it gives nothing.
    pub fn is_empty(&self) -> bool {
        self.keywords.iter().all(|k| k.trim().is_empty())
            && self.rating.is_none()
            && self.label.is_none()
    }

    /// As the fields an import sets ([`Metadata::commands`]).
    pub fn metadata(&self) -> Metadata {
        Metadata {
            rating: self.rating.map(|r| r.min(5)),
            labels: self.label.iter().cloned().collect(),
            tags: self
                .keywords
                .iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect(),
            ..Metadata::default()
        }
    }

    /// Keywords typed as one line, comma-separated.
    pub fn parse_keywords(line: &str) -> Vec<String> {
        line.split(',')
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(String::from)
            .collect()
    }
}

/// Reads an import's `filter`, `apply` and `stack_pairs` arguments into
/// `options`; absent or null, nothing changes.
///
/// # Errors
/// An argument that is not a filter or an apply, or a label not known.
pub fn read(args: &Value, options: &mut Options) -> Result<(), String> {
    if let Some(filter) = args.get("filter").filter(|v| !v.is_null()) {
        let filter: Filter =
            serde_json::from_value(filter.clone()).map_err(|e| format!("`filter`: {e}"))?;
        options.filter = (!filter.is_all()).then_some(filter);
    }
    if let Some(apply) = args.get("apply").filter(|v| !v.is_null()) {
        let apply: Apply =
            serde_json::from_value(apply.clone()).map_err(|e| format!("`apply`: {e}"))?;
        if let Some(label) = &apply.label
            && !LABELS.contains(&label.as_str())
        {
            return Err(format!("`apply`: `{label}` is not a colour label"));
        }
        options.apply = (!apply.is_empty()).then_some(apply);
    }
    if let Some(stack) = args.get("stack_pairs").and_then(Value::as_bool) {
        options.stack_pairs = stack;
    }
    Ok(())
}

/// The `filter`, `apply` and `stack_pairs` arguments of `options`, set
/// into `choices` when they say something (pairs left apart only) (the import history's record).
pub fn write(options: &Options, choices: &mut Value) {
    if let Some(filter) = &options.filter {
        choices["filter"] = json!(filter);
    }
    if let Some(apply) = &options.apply {
        choices["apply"] = json!(apply);
    }
    if !options.stack_pairs {
        choices["stack_pairs"] = json!(false);
    }
}

/// The schema of `library.import`'s `filter` argument.
pub fn filter_schema() -> Value {
    json!({
        "type": ["object", "null"],
        "description": "which files are taken: `kinds` (raw, jpeg, video, other), `names` (patterns with * and ?), `from` and `to` (YYYY-MM-DD), `min_bytes`, `max_bytes`, `new_only`",
    })
}

/// The schema of `library.import`'s `stack_pairs` argument.
pub fn stack_pairs_schema() -> Value {
    json!({
        "type": ["boolean", "null"],
        "description": "RAW+JPEG pairs of one shot stacked, the raw on top (on when absent)",
    })
}

/// The schema of `library.import`'s `apply` argument.
pub fn apply_schema() -> Value {
    json!({
        "type": ["object", "null"],
        "description": "what every photo imported is given: `keywords`, `rating` (0 to 5), `label` (red, yellow, green, blue, purple)",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_profile_a_filter_takes_by_kind_name_size_and_day() {
        let all = Filter::default();
        assert!(all.is_all());
        assert!(all.takes(Path::new("/c/IMG_1.CR3"), 1, None));
        let raw = Filter {
            kinds: vec![Kind::Raw],
            ..Filter::default()
        };
        assert!(raw.takes(Path::new("/c/IMG_1.CR3"), 1, None));
        assert!(!raw.takes(Path::new("/c/IMG_1.JPG"), 1, None));
        let named = Filter {
            names: vec!["img_*".into(), "*.nef".into()],
            ..Filter::default()
        };
        assert!(named.takes(Path::new("/c/IMG_0001.JPG"), 1, None));
        assert!(named.takes(Path::new("/c/DSC_1.NEF"), 1, None));
        assert!(!named.takes(Path::new("/c/DSC_1.JPG"), 1, None));
        let sized = Filter {
            min_bytes: Some(10),
            max_bytes: Some(20),
            ..Filter::default()
        };
        assert!(!sized.takes(Path::new("a.jpg"), 9, None));
        assert!(sized.takes(Path::new("a.jpg"), 20, None));
        assert!(!sized.takes(Path::new("a.jpg"), 21, None));
        // 2026-11-14 is day 20_771 after the epoch.
        let day = 20_771 * 86_400 + 3_600;
        let dated = Filter {
            from: Some("2026-11-14".into()),
            to: Some("2026-11-14".into()),
            ..Filter::default()
        };
        assert!(dated.takes(Path::new("a.jpg"), 1, Some(day)));
        assert!(!dated.takes(Path::new("a.jpg"), 1, Some(day + 86_400)));
        assert!(!dated.takes(Path::new("a.jpg"), 1, None));
    }

    #[test]
    fn import_profile_read_and_write_round_trip_through_the_choices() {
        let mut options = Options::default();
        read(
            &json!({
                "filter": { "kinds": ["jpeg"], "names": ["IMG_*"] },
                "apply": { "keywords": ["trip", " "], "rating": 3, "label": "red" },
            }),
            &mut options,
        )
        .unwrap();
        assert_eq!(options.filter.as_ref().unwrap().kinds, vec![Kind::Jpeg]);
        let apply = options.apply.clone().unwrap();
        assert_eq!(apply.metadata().tags, vec!["trip".to_string()]);
        assert_eq!(apply.metadata().labels, vec!["red".to_string()]);
        let mut choices = json!({});
        write(&options, &mut choices);
        assert_eq!(choices["filter"]["names"], json!(["IMG_*"]));
        assert_eq!(choices["apply"]["rating"], 3);
        // Empty ones are none; a label not known is refused.
        let mut options = Options::default();
        read(&json!({ "filter": {}, "apply": {} }), &mut options).unwrap();
        assert!(options.filter.is_none() && options.apply.is_none());
        assert!(read(&json!({ "apply": { "label": "pink" } }), &mut options).is_err());
        // Pairs are stacked unless told not to; off, the record says so.
        let mut options = Options::default();
        assert!(options.stack_pairs);
        read(&json!({ "stack_pairs": false }), &mut options).unwrap();
        assert!(!options.stack_pairs);
        let mut choices = json!({});
        write(&options, &mut choices);
        assert_eq!(choices["stack_pairs"], false);
        let mut again = Options::default();
        read(&choices, &mut again).unwrap();
        assert!(!again.stack_pairs);
    }
}
