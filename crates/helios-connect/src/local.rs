//! What the folder and card connectors share: a source on a local file
//! system, walked for the files Helios reads (`IMAGE_EXTENSIONS`), from
//! `DCIM` when the source holds one (the DCF rule), in a stable order.
//!
//! Items are the files, their id the path relative to the source, their
//! group the DCF object they belong to (same folder, same base name: a
//! RAW+JPEG pair, a THM). The cursor is the last id listed, so a listing
//! resumes after it whatever instance of the connector reads it, and the
//! walk is kept between the pages of one listing.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use helios_vault::IMAGE_EXTENSIONS;

use crate::{
    Access, Cancel, Caps, ConnectorError, ConnectorInfo, Cursor, FetchSink, Fetched, Item, ItemId,
    ItemKind, ItemRef, Page, SourceRef, Text,
};

/// What a source is: `card` (a folder holding `DCIM`, read from there),
/// `folder` or `file`; and where to read.
pub fn source_kind(source: &Path) -> (&'static str, PathBuf) {
    if source.is_file() {
        return ("file", source.to_path_buf());
    }
    let dcim = source.join("DCIM");
    if dcim.is_dir() {
        ("card", dcim)
    } else {
        ("folder", source.to_path_buf())
    }
}

/// Whether Helios reads the file, by its extension.
pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// The image files of a source, in a stable order.
pub fn files_of(source: &Path) -> Result<Vec<PathBuf>, String> {
    walk(source, &Cancel::new())
}

/// [`files_of`], stopped when `cancel` is set.
fn walk(source: &Path, cancel: &Cancel) -> Result<Vec<PathBuf>, String> {
    let (_, root) = source_kind(source);
    if root.is_file() {
        return if is_image(&root) {
            Ok(vec![root])
        } else {
            Err(format!("{}: not an image file", root.display()))
        };
    }
    let mut found = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() && is_image(&path) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// A file's kind, as the loader tells pictures from raws.
pub fn kind_of(path: &Path) -> ItemKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("jpg" | "jpeg" | "png" | "tif" | "tiff") => ItemKind::Photo,
        Some("xmp" | "thm") => ItemKind::Sidecar,
        _ => ItemKind::Raw,
    }
}

/// An id: the relative path, `/` between its parts on every system.
fn id_of(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The DCF object a file belongs to: its folder and its base name, case
/// left out (`DCIM/100CANON/img_0412`).
pub fn group_of(id: &str) -> String {
    let path = Path::new(id);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match path.parent().map(id_of).filter(|p| !p.is_empty()) {
        Some(parent) => format!("{parent}/{stem}"),
        None => stem,
    }
}

/// The folder ids are relative to: the source, or a file source's folder.
fn base_of(source: &SourceRef) -> PathBuf {
    let root = source
        .root
        .clone()
        .unwrap_or_else(|| PathBuf::from(&source.id));
    if root.is_file() {
        root.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        root
    }
}

/// A source at a local path, for a connector.
pub fn source_at(connector: &str, root: &Path, label: String, capacity: Option<u64>) -> SourceRef {
    SourceRef {
        connector: connector.to_owned(),
        id: root.to_string_lossy().into_owned(),
        label,
        root: Some(root.to_path_buf()),
        capacity,
    }
}

/// The last walk, kept for the next page of the same listing.
type Walked = Option<(PathBuf, Arc<Vec<PathBuf>>)>;

/// A connector over local sources; the folder and card connectors are
/// this with their own id, label, capabilities and sources.
pub(crate) struct Local {
    pub id: &'static str,
    pub label: Text,
    pub caps: Caps,
    walked: Mutex<Walked>,
}

impl Local {
    pub fn new(id: &'static str, label: Text, caps: Caps) -> Self {
        Self {
            id,
            label,
            caps,
            walked: Mutex::new(None),
        }
    }

    pub fn describe(&self) -> ConnectorInfo {
        ConnectorInfo {
            id: self.id.to_owned(),
            label: self.label.clone(),
            caps: self.caps.clone(),
        }
    }

    /// The source's files: walked anew for a listing from the start, the
    /// last walk for its next pages.
    fn files(
        &self,
        source: &SourceRef,
        again: bool,
        cancel: &Cancel,
    ) -> Result<Arc<Vec<PathBuf>>, ConnectorError> {
        let root = source
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from(&source.id));
        let mut walked = self.walked.lock().unwrap_or_else(PoisonError::into_inner);
        if again
            && let Some((at, files)) = walked.as_ref()
            && *at == root
        {
            return Ok(files.clone());
        }
        let files = match walk(&root, cancel) {
            Ok(f) => Arc::new(f),
            Err(_) if cancel.is_cancelled() => return Err(ConnectorError::Cancelled),
            Err(_) if !root.exists() => return Err(ConnectorError::Gone),
            Err(e) => return Err(ConnectorError::Transient(e)),
        };
        *walked = Some((root, files.clone()));
        Ok(files)
    }

    pub fn list(
        &self,
        source: &SourceRef,
        from: Option<&Cursor>,
        page: usize,
        cancel: &Cancel,
    ) -> Result<Page, ConnectorError> {
        let files = self.files(source, from.is_some(), cancel)?;
        let base = base_of(source);
        let rel = |p: &PathBuf| p.strip_prefix(&base).unwrap_or(p).to_path_buf();
        let start = from.map_or(0, |c| {
            let after = Path::new(&c.0);
            files.partition_point(|p| rel(p).as_path() <= after)
        });
        let page = page.max(1);
        let end = (start + page).min(files.len());
        let mut items = Vec::with_capacity(end - start);
        let mut last = None;
        for path in &files[start..end] {
            if cancel.is_cancelled() {
                return Err(ConnectorError::Cancelled);
            }
            let id = id_of(&rel(path));
            last = Some(id.clone());
            // A file gone or unreadable since the walk is left out.
            let Ok(meta) = std::fs::metadata(path) else {
                continue;
            };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_secs()).ok());
            items.push(Item {
                group: Some(group_of(&id)),
                path: id.clone(),
                id: ItemId(id),
                size: Some(meta.len()),
                modified,
                taken: None,
                kind: kind_of(path),
                mime: None,
                metadata: std::collections::BTreeMap::new(),
                claim: None,
                revision: None,
            });
        }
        let complete = end >= files.len();
        if complete {
            // The listing is through: its walk is not kept.
            let mut walked = self.walked.lock().unwrap_or_else(PoisonError::into_inner);
            if walked.as_ref().is_some_and(|(_, f)| Arc::ptr_eq(f, &files)) {
                *walked = None;
            }
        }
        Ok(Page {
            items,
            next: if complete { None } else { last.map(Cursor) },
            complete,
        })
    }

    /// The item's file: the source's path joined with its id.
    pub fn path_of(item: &ItemRef) -> Option<PathBuf> {
        let base = base_of(&item.source);
        let id = Path::new(&item.id.0);
        // An id never climbs out of its source.
        if id.is_absolute()
            || id
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return None;
        }
        Some(base.join(id))
    }

    pub fn access(item: &ItemRef) -> Access {
        Self::path_of(item).map_or(Access::Fetch, Access::InPlace)
    }

    pub fn fetch(
        item: &ItemRef,
        offset: u64,
        sink: &mut dyn FetchSink,
        cancel: &Cancel,
    ) -> Result<Fetched, ConnectorError> {
        let path = Self::path_of(item).ok_or(ConnectorError::NotFound)?;
        let mut file = File::open(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ConnectorError::Gone,
            _ => ConnectorError::Transient(format!("{}: {e}", path.display())),
        })?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| ConnectorError::Transient(format!("{}: {e}", path.display())))?;
        let mut buffer = vec![0u8; 1 << 16];
        let mut bytes = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(ConnectorError::Cancelled);
            }
            let n = file
                .read(&mut buffer)
                .map_err(|e| ConnectorError::Transient(format!("{}: {e}", path.display())))?;
            if n == 0 {
                break;
            }
            sink.accept(&buffer[..n])
                .map_err(|e| ConnectorError::Fatal(format!("{}: {e}", path.display())))?;
            bytes += n as u64;
        }
        Ok(Fetched { bytes, mime: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dcf_object_is_its_folder_and_base_name_case_left_out() {
        assert_eq!(
            group_of("DCIM/100CANON/IMG_0412.CR3"),
            "DCIM/100CANON/img_0412"
        );
        assert_eq!(
            group_of("DCIM/100CANON/img_0412.jpg"),
            "DCIM/100CANON/img_0412"
        );
        assert_eq!(group_of("a.jpg"), "a");
        assert_ne!(
            group_of("DCIM/101CANON/IMG_0412.JPG"),
            group_of("DCIM/100CANON/IMG_0412.JPG")
        );
    }

    #[test]
    fn kinds_follow_the_loader_pictures_and_raws() {
        assert_eq!(kind_of(Path::new("a.CR3")), ItemKind::Raw);
        assert_eq!(kind_of(Path::new("a.JPG")), ItemKind::Photo);
        assert_eq!(kind_of(Path::new("a.tif")), ItemKind::Photo);
    }
}
