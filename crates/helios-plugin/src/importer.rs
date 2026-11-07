//! The importer protocol (see `docs/extensions.md`, "Importers"): a
//! plugin lists what a source holds (`importer.scan`), Helios checks the
//! answer here and imports the files itself, so a plugin never writes the
//! library.
//!
//! - [`check_source`]: the file or folder the user picked, held to what
//!   the importer declares, before the plugin starts;
//! - [`checked`]: the plugin's answer, each image's path resolved and
//!   held to the source, its metadata to the library's fields; what does
//!   not hold is refused or dropped, and said.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value as Json;

use crate::manifest::{ImporterDecl, SourceKind};

/// The colour labels an image may carry.
pub const LABELS: &[&str] = &["red", "yellow", "green", "blue", "purple"];

/// An error code a plugin answers when the source is not one it reads
/// (the message is shown to the user).
pub const NOT_A_SOURCE: i64 = -32001;

/// The code of a request cancelled (LSP's `RequestCancelled`).
pub const CANCELLED: i64 = -32800;

/// One image the plugin found, checked.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Found {
    /// Absolute, symbolic links resolved.
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejected: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A path of the answer left out, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refused {
    pub path: String,
    pub why: String,
}

/// What a scan found, checked.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Scan {
    pub plugin: String,
    pub importer: String,
    /// The source, as picked (absolute, links resolved).
    pub source: PathBuf,
    pub images: Vec<Found>,
    pub refused: Vec<Refused>,
    /// The plugin's own warnings, then the values dropped.
    pub warnings: Vec<String>,
}

/// The picked source, absolute, when it is what the importer reads: a
/// folder holding what it must, or a file whose name it accepts.
pub fn check_source(decl: &ImporterDecl, source: &Path) -> Result<PathBuf, String> {
    let path = source
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    match decl.source {
        SourceKind::Folder => {
            if !path.is_dir() {
                return Err(format!("{} is not a folder", path.display()));
            }
            let missing: Vec<&str> = decl
                .holds
                .iter()
                .filter(|h| !path.join(h).exists())
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(format!(
                    "{} holds no {}: not a folder this importer reads",
                    path.display(),
                    missing.join(", ")
                ));
            }
        }
        SourceKind::File => {
            if !path.is_file() {
                return Err(format!("{} is not a file", path.display()));
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !decl.accepts(name) {
                return Err(format!(
                    "{name}: not a file this importer reads ({})",
                    decl.patterns.join(", ")
                ));
            }
        }
    }
    Ok(path)
}

/// The folder relative paths are read from, and what the images of an
/// importer that is not a catalogue must lie under: the source, or the
/// folder of a source file.
fn base(decl: &ImporterDecl, source: &Path) -> PathBuf {
    match decl.source {
        SourceKind::Folder => source.to_path_buf(),
        SourceKind::File => source.parent().unwrap_or(source).to_path_buf(),
    }
}

/// Checks a scan's answer: `images` (each a `path`, absolute or relative
/// to the source's folder, with optional `rating`, `rejected`, `labels`,
/// `tags`, `title`, `description`; other keys ignored) and `warnings`.
/// An image is refused when its path is missing, not a plain file, not
/// an image (`is_image`), outside the source (unless the importer reads a
/// catalogue, `anywhere`) or listed twice; a value that does not fit its
/// field is dropped with a warning, the image kept.
pub fn checked(
    answer: &Json,
    decl: &ImporterDecl,
    source: &Path,
    is_image: &dyn Fn(&Path) -> bool,
) -> Result<Scan, String> {
    let list = answer
        .get("images")
        .and_then(Json::as_array)
        .ok_or("the scan's answer has no `images` list")?;
    let mut scan = Scan {
        importer: decl.id.clone(),
        source: source.to_path_buf(),
        ..Scan::default()
    };
    if let Some(w) = answer.get("warnings").and_then(Json::as_array) {
        scan.warnings
            .extend(w.iter().filter_map(Json::as_str).map(String::from));
    }
    let base = base(decl, source);
    let mut seen = BTreeSet::new();
    for item in list {
        let given = item.get("path").and_then(Json::as_str).unwrap_or_default();
        let refuse = |why: &str| Refused {
            path: given.into(),
            why: why.into(),
        };
        if given.is_empty() {
            scan.refused.push(refuse("no path"));
            continue;
        }
        let path = match base.join(given).canonicalize() {
            Ok(p) => p,
            Err(e) => {
                scan.refused.push(refuse(&e.to_string()));
                continue;
            }
        };
        if !path.is_file() {
            scan.refused.push(refuse("not a plain file"));
        } else if !decl.anywhere && !path.starts_with(&base) {
            scan.refused.push(refuse("outside the source"));
        } else if !is_image(&path) {
            scan.refused.push(refuse("not an image file"));
        } else if !seen.insert(path.clone()) {
            scan.refused.push(refuse("listed twice"));
        } else {
            let found = fields(item, path, &mut scan.warnings);
            scan.images.push(found);
        }
    }
    Ok(scan)
}

/// An image's values, each held to its field; what does not fit is
/// dropped and said in `warnings`.
fn fields(item: &Json, path: PathBuf, warnings: &mut Vec<String>) -> Found {
    let mut found = Found {
        path,
        ..Found::default()
    };
    let at = found.path.display().to_string();
    let mut dropped = |what: &str, v: &Json| warnings.push(format!("{at}: {what} {v} dropped"));
    match item.get("rating") {
        None | Some(Json::Null) => {}
        Some(v) => match v.as_u64().filter(|r| *r <= 5) {
            Some(r) => found.rating = Some(r as u8),
            None => dropped("rating (0 to 5)", v),
        },
    }
    match item.get("rejected") {
        None | Some(Json::Null) => {}
        Some(Json::Bool(b)) => found.rejected = Some(*b),
        Some(v) => dropped("rejected (true or false)", v),
    }
    for (key, out) in [
        ("title", &mut found.title),
        ("description", &mut found.description),
    ] {
        match item.get(key) {
            Some(Json::String(s)) if !s.trim().is_empty() => *out = Some(s.clone()),
            None | Some(Json::Null | Json::String(_)) => {}
            Some(v) => dropped(key, v),
        }
    }
    let names = |key: &str| -> Vec<Json> {
        match item.get(key) {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(a)) => a.clone(),
            Some(v) => vec![v.clone()],
        }
    };
    for v in names("labels") {
        match v.as_str().map(str::to_lowercase) {
            Some(l) if LABELS.contains(&l.as_str()) => {
                if !found.labels.contains(&l) {
                    found.labels.push(l);
                }
            }
            _ => dropped("label", &v),
        }
    }
    for v in names("tags") {
        match v.as_str().map(str::trim) {
            Some(t) if !t.is_empty() => {
                if !found.tags.iter().any(|x| x == t) {
                    found.tags.push(t.into());
                }
            }
            _ => dropped("tag", &v),
        }
    }
    found
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    fn decl(source: SourceKind, anywhere: bool) -> ImporterDecl {
        ImporterDecl {
            id: "t".into(),
            label: "T".into(),
            source,
            patterns: Vec::new(),
            holds: Vec::new(),
            anywhere,
        }
    }

    fn jpeg(p: &Path) -> bool {
        p.extension().is_some_and(|e| e == "jpg")
    }

    #[test]
    fn an_answer_is_held_to_the_source_and_the_fields() {
        let root = std::env::temp_dir().join(format!("helios-importer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        for f in ["a.jpg", "sub/b.jpg", "notes.txt"] {
            std::fs::write(src.join(f), f).unwrap();
        }
        std::fs::write(root.join("outside.jpg"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("outside.jpg"), src.join("link.jpg")).unwrap();
        let src = src.canonicalize().unwrap();
        let answer = json!({
            "images": [
                {"path": "a.jpg", "rating": 4, "labels": ["Red", "teal"], "tags": [" trip ", ""], "extra": 1},
                {"path": src.join("sub/b.jpg"), "rating": 9, "rejected": true, "title": "B"},
                {"path": "a.jpg"},
                {"path": "notes.txt"},
                {"path": "missing.jpg"},
                {"path": "link.jpg"},
                {"path": "../outside.jpg"},
                {"rating": 1},
            ],
            "warnings": ["one sidecar unreadable"],
        });
        let scan = checked(&answer, &decl(SourceKind::Folder, false), &src, &jpeg).unwrap();
        assert_eq!(scan.images.len(), 2, "{scan:?}");
        let a = &scan.images[0];
        assert_eq!(
            (a.rating, &a.labels, &a.tags),
            (Some(4), &vec!["red".to_string()], &vec!["trip".to_string()])
        );
        let b = &scan.images[1];
        assert_eq!(
            (b.rating, b.rejected, b.title.as_deref()),
            (None, Some(true), Some("B"))
        );
        let why: Vec<&str> = scan.refused.iter().map(|r| r.why.as_str()).collect();
        assert_eq!(why.len(), 6, "{why:?}");
        assert!(why.contains(&"listed twice"), "{why:?}");
        assert!(why.contains(&"not an image file"), "{why:?}");
        assert_eq!(
            why.iter().filter(|w| **w == "outside the source").count(),
            2
        );
        assert!(why.contains(&"no path"), "{why:?}");
        assert_eq!(scan.warnings[0], "one sidecar unreadable");
        assert_eq!(scan.warnings.len(), 4, "{:?}", scan.warnings);
        // A catalogue names images anywhere.
        let scan = checked(&answer, &decl(SourceKind::Folder, true), &src, &jpeg).unwrap();
        assert_eq!(scan.images.len(), 3, "{scan:?}");
        assert!(checked(&json!({}), &decl(SourceKind::Folder, false), &src, &jpeg).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_source_is_what_the_importer_reads() {
        let root = std::env::temp_dir().join(format!("helios-importer-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("library.db"), "").unwrap();
        let mut folder = decl(SourceKind::Folder, true);
        folder.holds = vec!["library.db".into()];
        assert!(check_source(&folder, &root).is_ok());
        folder.holds.push("data.db".into());
        let e = check_source(&folder, &root).unwrap_err();
        assert!(e.contains("data.db"), "{e}");
        assert!(check_source(&folder, &root.join("library.db")).is_err());
        let mut file = decl(SourceKind::File, true);
        file.patterns = vec!["*.db".into()];
        assert!(check_source(&file, &root.join("library.db")).is_ok());
        file.patterns = vec!["*.lrcat".into()];
        assert!(check_source(&file, &root.join("library.db")).is_err());
        assert!(check_source(&file, &root.join("nothing")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
